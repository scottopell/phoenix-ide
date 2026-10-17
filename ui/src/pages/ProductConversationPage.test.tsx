import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { StrictMode, useEffect, useLayoutEffect } from 'react';
import { MemoryRouter, Route, Routes, useNavigate, useLocation } from 'react-router-dom';
import { ProductConversationPage } from './ProductConversationPage';
import { ViewJourneyLifecycle, defaultViewIdentity } from '../hooks/conversationViewLifecycle';
import { ViewJourneyReporter } from '../hooks/conversationViewTelemetry';
import { ProductConversationAliasRedirect } from '../App';
import { FocusScopeProvider } from '../hooks/useFocusScope';
import { transcriptPositioningCommandKey, type TranscriptPositioningCompletion, type TranscriptPositioningInput } from '../conversation/transcriptPositioning';
const actualHistory = vi.hoisted(() => ({ enabled: false, handled: vi.fn() }));
const historyResizeObservers: Array<{ callback: ResizeObserverCallback; elements: Set<Element> }> = [];
function observeHistoryGeometry() {
  act(() => {
    for (const observer of historyResizeObservers) observer.callback(
      [...observer.elements].map(target => ({ target, contentRect: target.getBoundingClientRect() }) as ResizeObserverEntry),
      {} as ResizeObserver,
    );
  });
}
function currentPositioning(): TranscriptPositioningInput {
  return conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning as TranscriptPositioningInput;
}
import { ConversationContext } from '../conversation/ConversationContext';
import { ConversationStore } from '../conversation/ConversationStore';
import type { Conversation, ConversationState } from '../api';
import { buildHistoricalUnits } from '../conversation/renderUnits';
import {
  ALIGNED_PREFIX_BOUNDARY_TOOL_ID,
  ALIGNED_PREFIX_STEERING_TOOL_ID,
  ALIGNED_PREFIX_TERMINAL_MARKER,
  getProductConversationScenario,
} from '../fixtures/productConversation/scenarios';
import { ConversationReadinessProvider } from '../contexts/ConversationReadinessContext';
import { FileExplorerProvider } from '../components/FileExplorer';
import { ViewerSlotProvider } from '../contexts/ViewerSlotContext';
import { ChainProvider } from '../chain';
import { ApiResponseError, type ChainView, type Message, type ProductConversationHistorySnapshotView as ProductConversationSnapshotView } from '../api';
import type { CoordinatorConversationSnapshotView } from '../generated/CoordinatorConversationSnapshotView';
import type { ProductConversationCloseView } from '../generated/ProductConversationCloseView';
import type { ConversationHistorySegmentView } from '../generated/ConversationHistorySegmentView';
import type { ProductConversationHandoffView } from '../generated/ProductConversationHandoffView';
import {
  notifyCloseSnapshotChanged,
  notifyProductConversationDeleted,
  notifyProductConversationSnapshotChanged,
  notifyProductConversationsReconciled,
} from '../notifications';

vi.mock('../hooks/conversationViewTelemetry', async importOriginal => {
  const actual = await importOriginal<typeof import('../hooks/conversationViewTelemetry')>();
  return { ...actual, ViewJourneyReporter: class extends actual.ViewJourneyReporter {
    constructor() { super({ sampleRate: 1, send: async () => new Response(null, { status: 204 }) }); }
  } };
});

const openingCommits: Message[][] = [];
const conversationNavStackSpy = vi.fn();
const embeddedConversationPageSpy = vi.fn();
const viewerSpy = vi.fn();
const chainQaColumnSpy = vi.fn();
class FakeCloseEventSource {
  static readonly OPEN = 1;
  static readonly CLOSED = 2;
  static instances: FakeCloseEventSource[] = [];
  readonly listeners = new Map<string, Array<(event: MessageEvent) => void>>();
  readyState = FakeCloseEventSource.OPEN;

  constructor(readonly url: string) {
    FakeCloseEventSource.instances.push(this);
  }

  addEventListener(type: string, listener: EventListenerOrEventListenerObject): void {
    const callback = listener as (event: MessageEvent) => void;
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), callback]);
  }

  close(): void {
    this.readyState = FakeCloseEventSource.CLOSED;
  }

  emit(type: string, payload: unknown): void {
    const event = new MessageEvent(type, { data: JSON.stringify(payload) });
    for (const listener of this.listeners.get(type) ?? []) listener(event);
  }
}
const viewportFlags = vi.hoisted(() => ({ isWideDesktop: true }));
const chainStream = vi.hoisted(() => {
  const close = vi.fn();
  const subscribe = vi.fn((...args: [
    string,
    (event: import('../api').ChainSseEventData) => void,
    (error: Event) => void,
  ]) => {
    void args;
    return { close };
  });
  return { close, subscribe };
});

vi.mock('../hooks/useMediaQuery', () => ({
  useIsWideDesktop: () => viewportFlags.isWideDesktop,
}));

vi.mock('../components/MessageViewer', () => ({
  MessageViewer: (props: Record<string, unknown>) => {
    viewerSpy(props);
    return <div data-testid="aggregate-message-viewer" data-inline={String(props['inline'])} />;
  },
}));

vi.mock('../components/ConversationNavStack', async () => {
  const actual = await vi.importActual<typeof import('../components/ConversationNavStack')>('../components/ConversationNavStack');
  return {
  ConversationNavStack: (props: Record<string, unknown>) => {
    conversationNavStackSpy(props);
    useLayoutEffect(() => { openingCommits.push(props['messages'] as Message[]); });
    if (actualHistory.enabled) {
      const typed = props as React.ComponentProps<typeof actual.ConversationNavStack>;
      return <FocusScopeProvider><actual.ConversationNavStack {...typed} onHistoryScrollCommandHandled={(...args) => {
        actualHistory.handled(...args);
        typed.onHistoryScrollCommandHandled?.(...args);
      }} /></FocusScopeProvider>;
    }
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
        <button onClick={() => {
          const input = props['transcriptPositioning'] as TranscriptPositioningInput;
          if (input.kind === 'positioning') (props['onHistoryScrollCommandHandled'] as ((completion: TranscriptPositioningCompletion) => void) | undefined)?.({
            commandKey: transcriptPositioningCommandKey(input.command), result: 'applied',
          });
        }}>
          complete positioning
        </button>
        <div data-testid="message-count">{messages.length}</div>
        <div data-testid="message-order">{messages.map((m) => m.message_id).join(',')}</div>
        <div data-testid="message-types">{messages.map((m) => m.message_type).join(',')}</div>
        <div data-testid="message-text-order">{messages.map((m) => typeof m.content === 'object' && m.content !== null && 'text' in m.content ? String((m.content as { text?: string }).text ?? '') : typeof m.content === 'object' && m.content !== null && 'summary' in m.content ? String((m.content as { summary?: string }).summary ?? '') : '').join('|')}</div>
      </div>
    );
  },
};
});

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
    const activeTextareaRef = props['activeTextareaRef'] as React.MutableRefObject<HTMLTextAreaElement | null>;
    const autoFocusActive = Boolean(props['autoFocusActive']);
    const disabled = Boolean(props['disabled']);
    const onActiveTextareaFocused = props['onActiveTextareaFocused'] as (() => void) | undefined;
    useEffect(() => {
      if (!autoFocusActive || disabled) return;
      activeTextareaRef.current?.focus();
      onActiveTextareaFocused?.();
    }, [activeTextareaRef, autoFocusActive, disabled, onActiveTextareaFocused]);
    const inflight = Array.isArray(props['inflight']) ? props['inflight'] as Array<{
      chainQaId: string;
      answer: string;
      error: string | null;
    }> : [];
    return (
      <div data-testid="chain-qa-column">
        <div data-testid="chain-qa-root">{String((props['chain'] as ChainView | undefined)?.root_conv_id ?? '')}</div>
        <div data-testid="chain-qa-persisted-count">{Array.isArray(props['persisted']) ? props['persisted'].length : 0}</div>
        <div data-testid="chain-qa-inflight">{inflight.map((entry) => `${entry.chainQaId}:${entry.answer}:${entry.error ?? ''}`).join('|')}</div>
        <form onSubmit={(event) => (props['onSubmit'] as ((event: React.FormEvent<HTMLFormElement>) => void) | undefined)?.(event)}>
          <textarea
            ref={activeTextareaRef}
            aria-label="recall draft"
            value={String(props['draft'] ?? '')}
            disabled={Boolean(props['disabled'])}
            onChange={(event) => (props['setDraft'] as ((value: string) => void))(event.target.value)}
          />
          <button type="submit" disabled={!props['onSubmit']}>Ask recall</button>
        </form>
        <button type="button" onClick={() => (props['onRetryConnection'] as (() => void) | undefined)?.()}>Retry recall connection</button>
        <button
          type="button"
          disabled={Boolean(props['disabled'])}
          onClick={() => (props['onReask'] as ((question: string) => void) | undefined)?.('What changed?')}
        >
          Re-ask recall
        </button>
        <div data-testid="chain-qa-sse-lost">{String(props['sseLost'])}</div>
      </div>
    );
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



vi.mock('../api', async () => {
  const actual = await vi.importActual<typeof import('../api')>('../api');
  return {
    ...actual,
    api: {
      ...actual.api,
      getProductConversationHistorySnapshot: vi.fn(),
      getProductConversationSnapshot: vi.fn(),
      getCoordinatorHistory: vi.fn(),
      resolveCoordinatorRoute: vi.fn(),
      getProductConversationAutomaticContinuation: vi.fn(),
      updateProductConversationAutomaticContinuation: vi.fn(),
      reportProductConversationOpen: vi.fn().mockResolvedValue(undefined),
      getPrStatus: vi.fn(),
      getChain: vi.fn(),
      submitChainQuestion: vi.fn(),
      approveTask: vi.fn(),
    },
    streamApi: {
      ...actual.streamApi,
      subscribeToChainStream: chainStream.subscribe,
    },
  };
});

function makeMessage(message_id: string, sequence_id: number, conversation_id = 'conv-a') {
  return {
    message_id,
    conversation_id,
    sequence_id,
    message_type: sequence_id % 2 === 0 ? 'agent' as const : 'user' as const,
    origin: { kind: 'unknown_historical' as const },
    content: { text: message_id },
    display_data: null,
    usage_data: null,
    created_at: `2026-01-01T00:00:${String(sequence_id).padStart(2, '0')}Z`,
  };
}

type SnapshotFixtureInput = Omit<ProductConversationSnapshotView, 'segments'> & {
  segments: Array<ConversationHistorySegmentView & { handoff?: ProductConversationHandoffView | null }>;
};
function makeSnapshot(overrides: Partial<SnapshotFixtureInput> = {}): ProductConversationSnapshotView {
  const snapshot: SnapshotFixtureInput = {
    product_conversation_id: 'pc-1',
    close: null,
    boundaries: [],
    canonical_route: '/product-conversations/pc-1',
    requested_transcript_row_id: 'row-2',
    canonical_root: { transcript_row_id: 'row-1', slug: 'root-slug', title: 'Root title' },
    ordinary_lifecycle: 'open',
    latest_transcript_row_id: 'row-2',
    writable_transcript_row_id: 'row-2',
    latest_opening: { status: 'none' },
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
        messages: [makeMessage('m-1', 1, 'row-1'), makeMessage('m-2', 2, 'row-1')],
        handoff: {
          kind: 'historical',
          predecessor_transcript_row_id: 'row-1',
          successor_transcript_row_id: 'row-2',
          continuation_message_id: 'cont-1',
          summary: 'First handoff',
        },
      },
      {
        segment_ordinal: 2,
        transcript_row_id: 'row-2',
        slug: 'row-2',
        title: 'Latest',
        messages: [makeMessage('m-3', 3, 'row-2'), makeMessage('m-4', 4, 'row-2')],
        handoff: null,
      },
    ],
    before: 'cursor-1',
    has_older: true,
    ...overrides,
  };
  snapshot.boundaries = overrides.boundaries ?? snapshot.segments.flatMap((segment) => segment.handoff ? [{ represented_successor_opening_message_id: null, message_id: segment.handoff.continuation_message_id, predecessor_transcript_row_id: segment.transcript_row_id, successor_transcript_row_id: segment.handoff.successor_transcript_row_id, segment_ordinal: segment.segment_ordinal }] : []);
  const segments = snapshot.segments.map(({ handoff, ...segment }) => !handoff || segment.messages.some((message) => message.message_id === handoff.continuation_message_id) ? segment : {
    ...segment, messages: [...segment.messages, { ...makeMessage(handoff.continuation_message_id, Math.max(0, ...segment.messages.map((message) => message.sequence_id)) + 1, segment.transcript_row_id), message_type: 'continuation' as const, content: { summary: handoff.summary } }],
  });
  return { ...snapshot, segments };
}

function makeChain(overrides: Partial<ChainView> = {}): ChainView {
  return {
    root_conv_id: 'root-chain',
    product_conversation_id: 'pc-1',
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
    work_identity: null,
    ...overrides,
  };
}

function ProductRouteSwitch() {
  const navigate = useNavigate();
  return <>
    <button onClick={() => navigate('/product-conversations/pc-1')}>open first product</button>
    <button onClick={() => navigate('/product-conversations/pc-2')}>open second product</button>
    <button onClick={() => navigate('/global/row-2')}>open second Global member</button>
    <button onClick={() => navigate('/global/row-4')}>open newly created Global member</button>
  </>;
}

function HistoryRouteControls() {
  const navigate = useNavigate();
  const location = useLocation();
  return <><div data-testid="history-location">{location.pathname}{location.search}{location.hash}</div>
    <button onClick={() => {
      const query = new URLSearchParams(location.search);
      if (query.has('source_transcript')) query.set('source_transcript', 'row-2');
      query.delete('source_tool');
      navigate({ ...location, search: query.toString(), hash: '#message-m-3' });
    }}>jump latest message</button>
    <button onClick={() => navigate({ ...location, hash: '' })}>clear message hash</button>
    <button onClick={() => navigate({ pathname: location.pathname, search: '', hash: '' }, { state: { chapterSelection: 'pending' } })}>select pending chapter</button>
    <button onClick={() => navigate(-1)}>history back</button>
    <button onClick={() => navigate(1)}>history forward</button>
  </>;
}

function pendingOpening(): import('../generated/ContinuationOpeningView').ContinuationOpeningView {
  return { status: 'pending', edge: { product_conversation_id: 'pc-1', predecessor_transcript_row_id: 'row-1', successor_transcript_row_id: 'row-2', continuation_message_id: 'cont-1' }, client_turn_key: 'reserved-key', opening_authority: 'generated_predecessor_context' };
}

function acceptedOpening(id: string, presentation: 'boundary' | 'separate_message'): import('../generated/ContinuationOpeningView').ContinuationOpeningView {
  const pending = pendingOpening();
  if (pending.status !== 'pending') throw new Error('fixture must be pending');
  return { status: 'accepted', edge: pending.edge, accepted_message_id: id, opening_authority: pending.opening_authority, presentation };
}

function coordinatorSnapshot(snapshot: ProductConversationSnapshotView) {
  return {
    product_conversation_id: snapshot.product_conversation_id,
    canonical_route: '/global/row-2', requested_transcript_row_id: 'row-1',
    canonical_root: snapshot.canonical_root, latest_transcript_row_id: snapshot.latest_transcript_row_id,
    writable_transcript_row_id: snapshot.writable_transcript_row_id, latest_opening: snapshot.latest_opening, updated_at: snapshot.updated_at,
    segments: snapshot.segments, boundaries: snapshot.boundaries, before: snapshot.before, has_older: snapshot.has_older,
  };
}

let conversationStore: ConversationStore;

function renderPage(initialEntry = '/product-conversations/pc-1', withRouteSwitch = false, strict = false, journey?: ViewJourneyLifecycle) {
  const content = (
    <ConversationContext.Provider value={conversationStore}>
    <ConversationReadinessProvider>
      <MemoryRouter initialEntries={[initialEntry]}>
        {actualHistory.enabled && <style>{'[hidden] { display: none; }'}</style>}
        <ViewerSlotProvider browserSessionActive={false}>
          <ChainProvider>
            <FileExplorerProvider>
            {withRouteSwitch && <ProductRouteSwitch />}
            <HistoryRouteControls />
            <Routes>
              <Route path="/c/:slug" element={<ProductConversationAliasRedirect reference="pc-1" />} />
              <Route path="/global/:slug" element={<ProductConversationPage journey={journey} productId="row-1" globalShell={{ composerQuickAction: undefined, stateBarExtension: undefined }} />} />
              <Route path="/product-conversations/:productConversationId" element={<ProductConversationPage journey={journey} />} />
            </Routes>
            </FileExplorerProvider>
          </ChainProvider>
        </ViewerSlotProvider>
      </MemoryRouter>
    </ConversationReadinessProvider>
    </ConversationContext.Provider>
  );
  return render(strict ? <StrictMode>{content}</StrictMode> : content);
}

function emitLatestProjection(overrides: Partial<Record<string, unknown>> = {}) {
  const props = embeddedConversationPageSpy.mock.lastCall?.[0] as Record<string, unknown> | undefined;
  const onProjectionChange = props?.['onProjectionChange'] as ((projection: Record<string, unknown> | null) => void) | undefined;
  if (!onProjectionChange) throw new Error('missing onProjectionChange');
  const projection = {
    slug: 'row-2',
    conversationId: 'row-2',
    conversation: { work_scope_key: 'ws-live' },
    messages: [makeMessage('live-sent', 5, 'row-2'), makeMessage('live-streamed', 6, 'row-2')],
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
    appendReviewNotesToComposer: vi.fn(),
    filePathRootDir: '/tmp/latest-root',
    systemPrompt: 'Preserve this system prompt',
    ...overrides,
  };
  const id = String(projection.conversationId);
  conversationStore.dispatch(id, {
    type: 'set_initial_data',
    reset: true,
    conversationId: id,
    conversation: { id, slug: projection.slug, updated_at: '2026-01-01', ...projection.conversation } as Conversation,
    messages: projection.messages as Message[],
    opening: (overrides['opening'] ?? null) as import('../generated/ContinuationOpeningView').ContinuationOpeningView | null,
    phase: projection.convState as ConversationState,
    contextWindow: { used: 0 },
  });
  const { messages: _messages, convState: _phase, ...controls } = projection;
  void _messages;
  void _phase;
  onProjectionChange(controls);
}

beforeEach(() => {
  conversationStore = new ConversationStore();
  actualHistory.enabled = false;
  openingCommits.length = 0;
  historyResizeObservers.length = 0;
  vi.clearAllMocks();
  localStorage.clear();
  conversationNavStackSpy.mockClear();
  embeddedConversationPageSpy.mockClear();
  viewerSpy.mockClear();
  chainQaColumnSpy.mockClear();
  chainStream.close.mockClear();
  chainStream.subscribe.mockClear();
  viewportFlags.isWideDesktop = true;
  FakeCloseEventSource.instances = [];
  vi.stubGlobal('EventSource', FakeCloseEventSource);
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

async function waitForPageReady() {
  await screen.findByTestId('message-order');
}

describe('ProductConversationPage', () => {
  it.each(['applied', 'superseded'] as const)('keeps a held hidden alias jump when an earlier page arrives before completion (%s)', async (terminal) => {
    const { api } = await import('../api');
    actualHistory.enabled = true;
    vi.stubGlobal('ResizeObserver', class {
      elements = new Set<Element>();
      constructor(readonly callback: ResizeObserverCallback) { historyResizeObservers.push(this); }
      observe(element: Element) { this.elements.add(element); }
      unobserve(element: Element) { this.elements.delete(element); }
      disconnect() { this.elements.clear(); }
    });
    const nativeTops = new WeakMap<HTMLElement, number>();
    vi.spyOn(HTMLElement.prototype, 'scrollTop', 'get').mockImplementation(function (this: HTMLElement) { return nativeTops.get(this) ?? 0; });
    vi.spyOn(HTMLElement.prototype, 'scrollTop', 'set').mockImplementation(function (this: HTMLElement, top: number) {
      if (!this.closest('[hidden]')) nativeTops.set(this, top);
    });
    vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockReturnValue(100);
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      return { top: 0, bottom: 20, height: this.id === 'messages' ? 100 : 20, left: 0, right: 400, width: 400, x: 0, y: 0, toJSON() {} };
    });
    let resolveOlder!: (page: ProductConversationSnapshotView) => void;
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(makeSnapshot())
      .mockReturnValueOnce(new Promise(resolve => { resolveOlder = resolve; }));
    vi.mocked(api.resolveCoordinatorRoute).mockResolvedValue({ coordinator_id: null });
    const aliasSnapshot = makeSnapshot({ canonical_route: '/c/pc-1' });
    vi.mocked(api.getProductConversationSnapshot).mockResolvedValue({ ...aliasSnapshot,
      segments: aliasSnapshot.segments.map(segment => ({ ...segment, handoff: null })),
    });
    renderPage('/c/pc-1');
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.onLoadOlderMessages).toBeTypeOf('function'));
    // Start acquisition with a reader viewport; then navigation takes ownership.
    await act(async () => {
      const load = conversationNavStackSpy.mock.lastCall?.[0]?.onLoadOlderMessages as (basis: unknown) => Promise<void>;
      void load({ kind: 'reader_viewport' });
    });
    const aliasWrapper = document.querySelector('.product-conversation-page')!.parentElement!;
    await waitFor(() => expect(aliasWrapper.hidden).toBe(false));
    aliasWrapper.hidden = true;
    fireEvent.click(screen.getByRole('button', { name: 'jump latest message', hidden: true }));
    const oldInput = currentPositioning();
    expect(oldInput).toMatchObject({ kind: 'positioning', command: { targetMessageId: 'm-3' } });
    expect(actualHistory.handled).not.toHaveBeenCalled();
    await act(async () => resolveOlder(makeSnapshot({ before: null, has_older: false,
      segments: [{ segment_ordinal: 0, transcript_row_id: 'older', slug: 'older', title: 'Older', messages: [makeMessage('old-message', 1, 'older')], handoff: null }],
    })));
    const replacement = currentPositioning();
    expect(replacement).toMatchObject({ kind: 'positioning', command: { targetMessageId: 'm-3', view: { generation: 1 } } });
    if (oldInput.kind !== 'positioning' || replacement.kind !== 'positioning') throw new Error('expected owned jump commands');
    expect(replacement.command.token).toBe(oldInput.command.token);
    expect(transcriptPositioningCommandKey(replacement.command)).not.toBe(transcriptPositioningCommandKey(oldInput.command));
    if (terminal === 'superseded') {
      fireEvent.wheel(document.getElementById('messages')!, { deltaY: 10 });
      expect(currentPositioning().kind).toBe('idle');
    }
    aliasWrapper.hidden = false;
    observeHistoryGeometry();
    observeHistoryGeometry();
    await waitFor(() => expect(currentPositioning().kind).toBe('idle'));
    expect(actualHistory.handled.mock.calls).toEqual([
      [{ commandKey: transcriptPositioningCommandKey(oldInput.command), result: 'superseded' }],
      [{ commandKey: transcriptPositioningCommandKey(replacement.command), result: terminal }],
    ]);
    if (terminal === 'applied') expect(document.getElementById('message-m-3')).not.toBeNull();
    expect(screen.getByTestId('history-location')).toHaveTextContent('/c/pc-1#message-m-3');
  });

  it('acknowledges the exact current missing target without accepting another semantic command', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValue(makeSnapshot({ before: null, has_older: false }));
    renderPage('/product-conversations/pc-1#message-m-3');
    await waitForPageReady();
    const input = currentPositioning();
    if (input.kind !== 'positioning' || input.command.kind !== 'jump_to_message') throw new Error('expected jump');
    const finish = conversationNavStackSpy.mock.lastCall?.[0]?.onHistoryScrollCommandHandled as (completion: TranscriptPositioningCompletion) => void;
    for (const other of [
      { ...input.command, targetMessageId: 'm-1' },
      { ...input.command, requestToken: 99 },
      { ...input.command, view: { ...input.command.view, transcriptGeneration: 99 } },
    ]) {
      act(() => finish({ commandKey: transcriptPositioningCommandKey(other), result: 'superseded' }));
      expect(currentPositioning()).toEqual(input);
    }
    act(() => finish({ commandKey: transcriptPositioningCommandKey(input.command), result: 'target_missing' }));
    expect(currentPositioning().kind).toBe('idle');
    fireEvent.click(screen.getByRole('button', { name: 'clear message hash' }));
    expect(currentPositioning().kind).toBe('idle');
  });

  it('uses aggregate Global history without ordinary lifecycle and keeps newest composer separate from pinned evidence', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getCoordinatorHistory).mockResolvedValue(coordinatorSnapshot(makeSnapshot({ before: null, has_older: false })));
    renderPage('/global/row-1?source_transcript=row-1&source_tool=exact-call#message-m-1');
    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent('m-1,m-2,cont-1,m-3,m-4'));
    expect(api.getProductConversationHistorySnapshot).not.toHaveBeenCalled();
    expect(screen.queryByText('Open')).not.toBeInTheDocument();
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]).toMatchObject({ slug: 'row-2', routePrefix: '/global' });
    expect(screen.getByTestId('history-location')).toHaveTextContent('/global/row-1?source_transcript=row-1&source_tool=exact-call#message-m-1');
    expect(conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning).toMatchObject({ kind: 'positioning', command: { targetMessageId: 'm-1' } });
  });

  it('loads an unloaded Global boundary by exact message ID through the same bounded page loader', async () => {
    const { api } = await import('../api');
    const complete = coordinatorSnapshot(makeSnapshot({ before: null, has_older: false }));
    const tail = { ...complete, segments: complete.segments.map((segment) => segment.transcript_row_id === 'row-1' ? { ...segment, messages: [] } : segment), before: 'global-before', has_older: true };
    vi.mocked(api.getCoordinatorHistory).mockResolvedValueOnce(tail).mockResolvedValueOnce(complete);
    renderPage('/global/row-1?source_transcript=row-1#message-cont-1');
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning).toMatchObject({ kind: 'positioning', command: { targetMessageId: 'cont-1' } }));
    expect(api.getCoordinatorHistory).toHaveBeenNthCalledWith(2, 'row-1', expect.objectContaining({ message_limit: 100, before: 'global-before', open_id: expect.any(String), onTelemetry: expect.any(Function) }));
    expect((conversationNavStackSpy.mock.lastCall?.[0]?.messages as Message[]).filter((message) => message.message_id === 'cont-1')).toHaveLength(1);
    expect(screen.getByTestId('history-location')).toHaveTextContent('?source_transcript=row-1#message-cont-1');
  });

  it.each([[false, 'user'], [true, 'user'], [false, 'continuation'], [true, 'continuation']] as const)('keeps a reserved opening read-only until its accepted signal refreshes readiness (Global=%s, opening=%s)', async (global, openingType) => {
    const { api } = await import('../api');
    const pending = makeSnapshot({ latest_opening: pendingOpening(), writable_transcript_row_id: null, before: null, has_older: false });
    pending.segments = pending.segments.map((segment) => segment.transcript_row_id === 'row-2' ? { ...segment, messages: [] } : segment);
    const opening = { ...makeMessage('accepted-opening', 1, 'row-2'), message_type: openingType, content: openingType === 'continuation' ? { type: 'continuation', summary: 'generated opening' } : { type: 'user', text: 'accepted opening' } };
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValueOnce(coordinatorSnapshot(pending));
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(pending);
    renderPage(global ? '/global/row-1' : '/product-conversations/pc-1');
    await waitFor(() => expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.mutationEnabled).toBe(false));
    emitLatestProjection({ messages: [opening], opening: acceptedOpening(opening.message_id, 'separate_message'), phase: { type: 'idle' } });
    await waitFor(() => expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.mutationEnabled).toBe(true));
    expect(global ? api.getCoordinatorHistory : api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(1);
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.slug).toBe('row-2');
  });

  it.each([false, true].flatMap(global => ['live', 'init', 'replay'].flatMap(mode => [false, true].map(prefixed => [global, mode, prefixed] as const))))('settles only typed opening authority without GET success (Global=%s, path=%s, prefixed=%s)', async (global, mode, prefixed) => {
    const { api } = await import('../api');
    const pending = makeSnapshot({ latest_opening: pendingOpening(), writable_transcript_row_id: null, before: null, has_older: false });
    pending.segments = pending.segments.map(segment => segment.transcript_row_id === 'row-2' ? { ...segment, messages: [] } : segment);
    const loader = global ? vi.mocked(api.getCoordinatorHistory) : vi.mocked(api.getProductConversationHistorySnapshot);
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValueOnce(coordinatorSnapshot(pending)).mockRejectedValueOnce(new Error('typed refresh failed'));
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(pending).mockRejectedValueOnce(new Error('typed refresh failed'));
    renderPage(global ? '/global/row-1?source_transcript=row-1' : '/product-conversations/pc-1?source_transcript=row-1');
    await waitForPageReady();
    const opening = { ...makeMessage(prefixed ? 'row-2:reserved-key' : 'reserved-key', 1, 'row-2'), content: { type: 'user', text: 'First handoff' } };
    const manual = { ...makeMessage('manual-equal-text', 2, 'row-2'), content: { type: 'user', text: 'First handoff' } };
    act(() => emitLatestProjection({ messages: [opening, manual] }));
    expect(screen.getByTestId('message-order')).toHaveTextContent('manual-equal-text');
    expect(screen.getByTestId('message-order')).not.toHaveTextContent('reserved-key');
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.mutationEnabled).toBe(false);
    const accepted = acceptedOpening(opening.message_id, 'boundary');
    act(() => {
      if (mode === 'live') conversationStore.dispatch('row-2', { type: 'sse_message', message: opening, sequenceId: 1, opening: accepted });
      else conversationStore.dispatch('row-2', { type: 'sse_init', payload: {
        conversation: conversationStore.getSnapshot('row-2').conversation!, messages: mode === 'init' ? [opening, manual] : [], steeringMessages: [],
        opening: mode === 'init' ? accepted : null, phase: { type: 'idle' }, contextWindow: { used: 0 }, transcriptGeneration: 1,
        streamIncarnation: 'typed-opening', lastAppliedEventSeq: mode === 'init' ? 2 : 1, pendingAnchorSequenceId: mode === 'init' ? 2 : 0,
        pendingEvents: mode === 'replay' ? [{ type: 'message', message: opening, sequence_id: 1, opening: accepted }] : [],
        pendingTruncated: false, transcriptCoverage: 'tail',
      } });
    });
    await waitFor(() => expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.mutationEnabled).toBe(true));
    expect(screen.getByTestId('message-order')).not.toHaveTextContent('reserved-key');
    expect(screen.getByTestId('message-order')).toHaveTextContent('manual-equal-text');
    act(() => notifyProductConversationSnapshotChanged('pc-1'));
    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('typed refresh failed'));
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.mutationEnabled).toBe(true);
    expect(loader).toHaveBeenCalledTimes(2);
    const floor = conversationStore.getSnapshot('row-2').lastAppliedEventSeq;
    act(() => conversationStore.dispatch('row-2', { type: 'sse_message_updated', sequenceId: floor + 1, messageId: opening.message_id,
      content: { text: 'Edited opening' }, transcriptGeneration: 2, opening: acceptedOpening(opening.message_id, 'separate_message') }));
    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent(opening.message_id));
    expect(screen.getByTestId('message-text-order')).toHaveTextContent('Edited opening');
    act(() => conversationStore.dispatch('row-2', { type: 'sse_message_updated', sequenceId: floor + 2, messageId: opening.message_id,
      content: { text: 'First handoff' }, transcriptGeneration: 3, opening: accepted }));
    await waitFor(() => expect(screen.getByTestId('message-order')).not.toHaveTextContent('reserved-key'));
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValue(coordinatorSnapshot(pending));
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValue(pending);
    act(() => notifyProductConversationSnapshotChanged('pc-1'));
    await waitFor(() => expect(loader).toHaveBeenCalledTimes(3));
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.mutationEnabled).toBe(true);
  });

  it.each(['history', 'closing'] as const)('does not grant newest readiness to a read-only %s aggregate', async lifecycle => {
    const { api } = await import('../api');
    const snapshot = makeSnapshot({ latest_opening: pendingOpening(), writable_transcript_row_id: null, before: null, has_older: false,
      ordinary_lifecycle: lifecycle === 'history' ? 'history' : 'open',
      close: lifecycle === 'closing' ? { attempt_id: 'closing', phase: 'settling_active_work', outcome: null, run_ordinal: '1', run_status: 'running', failure: null, confirmation_snapshot: null, inspections: [], losses: [], residuals: [] } : null });
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValue(snapshot);
    renderPage('/product-conversations/pc-1');
    await waitForPageReady();
    act(() => conversationStore.dispatch('row-2', { type: 'set_initial_data', conversationId: 'row-2', conversation: { id: 'row-2', slug: 'row-2' } as Conversation, messages: [makeMessage('accepted-readonly', 1, 'row-2')], opening: acceptedOpening('accepted-readonly', 'boundary'), phase: { type: 'idle' }, contextWindow: { used: 0 } }));
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.mutationEnabled).not.toBe(true);
  });

  it.each([false, true].flatMap(global => (['boundary', 'separate_message'] as const).map(presentation => [global, presentation] as const)))('keeps opening content and presentation coherent before passive effects (Global=%s, presentation=%s)', async (global, presentation) => {
    const { api } = await import('../api');
    const id = 'coherent-opening';
    const initial = makeSnapshot({ latest_opening: acceptedOpening(id, presentation === 'boundary' ? 'separate_message' : 'boundary'), before: null, has_older: false });
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValue(coordinatorSnapshot(initial));
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValue(initial);
    renderPage(global ? '/global/row-1' : '/product-conversations/pc-1');
    await waitForPageReady();
    act(() => emitLatestProjection({ opening: initial.latest_opening, messages: [makeMessage(id, 1, 'row-2')] }));
    openingCommits.length = 0;
    act(() => conversationStore.dispatch('row-2', { type: 'sse_message_updated', sequenceId: 1, messageId: id,
      content: { text: 'COMMITTED_EDIT' }, transcriptGeneration: 2, opening: acceptedOpening(id, presentation) }));
    expect(openingCommits.length).toBeGreaterThan(0);
    for (const messages of openingCommits) {
      const rows = messages.filter(message => message.message_id === id);
      expect(rows).toHaveLength(presentation === 'boundary' ? 0 : 1);
      if (presentation === 'separate_message') expect(rows[0]?.content).toEqual({ text: 'COMMITTED_EDIT' });
    }
  });

  it.each([false, true].flatMap(global => (['boundary', 'separate_message'] as const).map(presentation => [global, presentation] as const)))('fences a delayed accepted GET but admits a later GET (Global=%s, presentation=%s)', async (global, presentation) => {
    const { api } = await import('../api');
    const id = 'causal-opening';
    const oldPresentation = presentation === 'boundary' ? 'separate_message' : 'boundary';
    const initial = makeSnapshot({ latest_opening: pendingOpening(), writable_transcript_row_id: null, before: null, has_older: false });
    const durable = makeSnapshot({ latest_opening: acceptedOpening(id, oldPresentation), before: null, has_older: false });
    durable.boundaries = durable.boundaries.map(boundary => ({ ...boundary, represented_successor_opening_message_id: oldPresentation === 'boundary' ? id : null }));
    durable.segments = durable.segments.map(segment => segment.transcript_row_id === 'row-2' ? { ...segment, messages: [{ ...makeMessage(id, 1, 'row-2'), content: { text: 'GET_CONTENT' } }] } : segment);
    let release!: () => void;
    const deferred = new Promise<void>(resolve => { release = resolve; });
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValueOnce(coordinatorSnapshot(initial)).mockImplementationOnce(async () => { await deferred; return coordinatorSnapshot(durable); }).mockResolvedValue(coordinatorSnapshot(durable));
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(initial).mockImplementationOnce(async () => { await deferred; return durable; }).mockResolvedValue(durable);
    const loader = global ? api.getCoordinatorHistory : api.getProductConversationHistorySnapshot;
    renderPage(global ? '/global/row-1' : '/product-conversations/pc-1');
    await waitForPageReady();
    act(() => emitLatestProjection({ opening: acceptedOpening(id, oldPresentation), messages: [makeMessage(id, 1, 'row-2')] }));
    act(() => notifyProductConversationSnapshotChanged('pc-1'));
    await waitFor(() => expect(loader).toHaveBeenCalledTimes(2));
    act(() => conversationStore.dispatch('row-2', { type: 'sse_message_updated', sequenceId: 1, messageId: id,
      content: { text: 'NEW_STREAM_CONTENT' }, transcriptGeneration: 2, opening: acceptedOpening(id, presentation) }));
    openingCommits.length = 0;
    await act(async () => { release(); await deferred; });
    expect(openingCommits.length).toBeGreaterThan(0);
    for (const messages of openingCommits) {
      const rows = messages.filter(message => message.message_id === id);
      expect(rows).toHaveLength(presentation === 'boundary' ? 0 : 1);
      if (presentation === 'separate_message') expect(rows[0]?.content).toEqual({ text: 'NEW_STREAM_CONTENT' });
    }
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.mutationEnabled).toBe(true);
    act(() => notifyProductConversationSnapshotChanged('pc-1'));
    await waitFor(() => expect(loader).toHaveBeenCalledTimes(3));
    await waitFor(() => expect(screen.getByTestId('message-order').textContent?.split(',').includes(id)).toBe(oldPresentation === 'separate_message'));
    if (oldPresentation === 'separate_message') expect(screen.getByTestId('message-text-order')).toHaveTextContent('GET_CONTENT');
  });

  it.each([false, true].flatMap(global => [null, { status: 'none' } as const].map(opening => [global, opening] as const)))('does not let absent opening evidence unlock pending readiness (Global=%s, opening=%s)', async (global, opening) => {
    const { api } = await import('../api');
    const pending = makeSnapshot({ latest_opening: pendingOpening(), writable_transcript_row_id: null, before: null, has_older: false });
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValue(coordinatorSnapshot(pending));
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValue(pending);
    renderPage(global ? '/global/row-1' : '/product-conversations/pc-1');
    await waitForPageReady();
    act(() => emitLatestProjection({ opening, messages: [makeMessage('reserved-key', 1, 'row-2')] }));
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.mutationEnabled).toBe(false);
    expect(screen.getByTestId('message-order')).not.toHaveTextContent('reserved-key');
  });

  it.each([false, true])('keeps later GET opening authority across unrelated transcript generations (Global=%s)', async global => {
    const { api } = await import('../api');
    const id = 'get-owned-opening';
    const pending = makeSnapshot({ latest_opening: pendingOpening(), writable_transcript_row_id: null, before: null, has_older: false });
    const ready = makeSnapshot({ latest_opening: acceptedOpening(id, 'separate_message'), before: null, has_older: false });
    ready.segments = ready.segments.map(segment => segment.transcript_row_id === 'row-2' ? { ...segment, messages: [{ ...makeMessage(id, 1, 'row-2'), content: { text: 'NEW_GET_CONTENT' } }] } : segment);
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValueOnce(coordinatorSnapshot(pending)).mockResolvedValue(coordinatorSnapshot(ready));
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(pending).mockResolvedValue(ready);
    renderPage(global ? '/global/row-1' : '/product-conversations/pc-1');
    await waitForPageReady();
    act(() => emitLatestProjection({ opening: acceptedOpening(id, 'boundary'), messages: [makeMessage(id, 1, 'row-2'), makeMessage('unrelated', 2, 'row-2')] }));
    act(() => notifyProductConversationSnapshotChanged('pc-1'));
    await waitFor(() => expect(screen.getByTestId('message-text-order')).toHaveTextContent('NEW_GET_CONTENT'));
    act(() => conversationStore.dispatch('row-2', { type: 'sse_message_updated', sequenceId: 1, messageId: 'unrelated', content: { text: 'UNRELATED_EDIT' }, transcriptGeneration: 3, opening: null }));
    expect(screen.getByTestId('message-text-order')).toHaveTextContent('NEW_GET_CONTENT');
    expect(screen.getByTestId('message-text-order')).toHaveTextContent('UNRELATED_EDIT');
  });

  it('does not use cached live opening authority when reopening requires fresh hydration', async () => {
    const { api } = await import('../api');
    const history = makeSnapshot({ ordinary_lifecycle: 'history', latest_opening: pendingOpening(), writable_transcript_row_id: null, before: null, has_older: false });
    const reopened = { ...history, ordinary_lifecycle: 'open' as const };
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(history).mockResolvedValue(reopened);
    renderPage('/product-conversations/pc-1');
    await waitForPageReady();
    act(() => conversationStore.dispatch('row-2', { type: 'set_initial_data', conversationId: 'row-2',
      conversation: { id: 'row-2', slug: 'row-2' } as Conversation, messages: [makeMessage('cached-opening', 1, 'row-2')],
      opening: acceptedOpening('cached-opening', 'boundary'), phase: { type: 'idle' }, contextWindow: { used: 0 } }));
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.mutationEnabled).not.toBe(true);
    act(() => notifyProductConversationSnapshotChanged('pc-1'));
    await waitFor(() => expect(conversationStore.getTranscriptAuthority('row-2').kind).toBe('awaiting_init'));
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.mutationEnabled).not.toBe(true);
    expect(screen.getByTestId('message-order')).not.toHaveTextContent('cached-opening');
  });

  it.each([false, true])('admits an initial GET started after a cached live opening (Global=%s)', async global => {
    const { api } = await import('../api');
    const id = 'cached-before-get';
    conversationStore.dispatch('row-2', { type: 'set_initial_data', conversationId: 'row-2',
      conversation: { id: 'row-2', slug: 'row-2' } as Conversation, messages: [makeMessage(id, 1, 'row-2')],
      opening: acceptedOpening(id, 'boundary'), phase: { type: 'idle' }, contextWindow: { used: 0 } });
    const ready = makeSnapshot({ latest_opening: acceptedOpening(id, 'separate_message'), before: null, has_older: false });
    ready.segments = ready.segments.map(segment => segment.transcript_row_id === 'row-2' ? { ...segment, messages: [{ ...makeMessage(id, 1, 'row-2'), content: { text: 'INITIAL_GET_CONTENT' } }] } : segment);
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValue(coordinatorSnapshot(ready));
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValue(ready);
    renderPage(global ? '/global/row-1' : '/product-conversations/pc-1');
    await waitForPageReady();
    expect(screen.getByTestId('message-text-order')).toHaveTextContent('INITIAL_GET_CONTENT');
  });

  it('permits an authoritative accepted GET presentation update after an earlier live receipt', async () => {
    const { api } = await import('../api');
    const pending = makeSnapshot({ latest_opening: pendingOpening(), writable_transcript_row_id: null, before: null, has_older: false });
    const id = 'opening-update';
    const ready = makeSnapshot({ latest_opening: acceptedOpening(id, 'separate_message'), before: null, has_older: false });
    ready.segments = ready.segments.map(segment => segment.transcript_row_id === 'row-2' ? { ...segment, messages: [makeMessage(id, 1, 'row-2')] } : segment);
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(pending).mockResolvedValueOnce(ready);
    renderPage('/product-conversations/pc-1');
    await waitForPageReady();
    act(() => emitLatestProjection({ opening: acceptedOpening(id, 'boundary'), messages: [makeMessage(id, 1, 'row-2')] }));
    await waitFor(() => expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.mutationEnabled).toBe(true));
    expect(screen.getByTestId('message-order')).not.toHaveTextContent(id);
    act(() => notifyProductConversationSnapshotChanged('pc-1'));
    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent(id));
  });

  it.each(['product_conversation_id', 'predecessor_transcript_row_id', 'successor_transcript_row_id', 'continuation_message_id'] as const)('rejects a foreign opening %s while preserving pending authority', async field => {
    const { api } = await import('../api');
    const pending = makeSnapshot({ latest_opening: pendingOpening(), writable_transcript_row_id: null, before: null, has_older: false });
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValue(pending);
    renderPage('/product-conversations/pc-1');
    await waitForPageReady();
    const accepted = acceptedOpening('foreign-opening', 'boundary');
    if (accepted.status !== 'accepted') throw new Error('fixture');
    act(() => emitLatestProjection({ messages: [makeMessage('foreign-opening', 1, 'row-2')], opening: { ...accepted, edge: { ...accepted.edge, [field]: 'foreign' } } }));
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.mutationEnabled).toBe(false);
    expect(screen.getByTestId('message-order')).toHaveTextContent('foreign-opening');
  });

  it.each([false, true])('rejects loaded evidence owned by another pinned transcript (Global=%s)', async global => {
    const { api } = await import('../api');
    const snapshot = makeSnapshot({ before: 'still-older', has_older: true });
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValue(coordinatorSnapshot(snapshot));
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValue(snapshot);
    renderPage(global ? '/global/row-1?source_transcript=row-1&source_tool=wrong-tool&keep=yes#message-m-3' : '/product-conversations/pc-1?source_transcript=row-1&source_tool=wrong-tool&keep=yes#message-m-3');
    await waitForPageReady();
    expect(screen.getByRole('alert')).toHaveTextContent('linked message is not available');
    expect(currentPositioning().kind).toBe('idle');
    expect(global ? api.getCoordinatorHistory : api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(1);
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.slug).toBe('row-2');
    expect(screen.getByTestId('history-location')).toHaveTextContent('source_transcript=row-1&source_tool=wrong-tool&keep=yes#message-m-3');
  });

  it.each([false, true])('pages unknown hash identity before rejecting a mismatched source pin (Global=%s)', async global => {
    const { api } = await import('../api');
    const initial = makeSnapshot();
    const page = { ...initial, before: null, has_older: false, segments: initial.segments.map(segment => segment.transcript_row_id === 'row-2' ? { ...segment, messages: [makeMessage('unloaded-wrong-owner', 1, 'row-2')] } : { ...segment, messages: [] }) };
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValueOnce(coordinatorSnapshot(initial)).mockResolvedValueOnce(coordinatorSnapshot(page));
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(initial).mockResolvedValueOnce(page);
    renderPage(global ? '/global/row-1?source_transcript=row-1#message-unloaded-wrong-owner' : '/product-conversations/pc-1?source_transcript=row-1#message-unloaded-wrong-owner');
    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent('unloaded-wrong-owner'));
    expect(screen.getByRole('alert')).toHaveTextContent('linked message is not available');
    expect(currentPositioning().kind).toBe('idle');
    expect(global ? api.getCoordinatorHistory : api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(2);
  });

  it.each([false, true])('rejects an opening alias pinned to its predecessor rather than its exact successor (Global=%s)', async global => {
    const { api } = await import('../api');
    const snapshot = makeSnapshot({ before: 'still-older', has_older: true, boundaries: [{ message_id: 'cont-1', represented_successor_opening_message_id: 'exact-opening', predecessor_transcript_row_id: 'row-1', successor_transcript_row_id: 'row-2', segment_ordinal: 1 }] });
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValue(coordinatorSnapshot(snapshot));
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValue(snapshot);
    renderPage(global ? '/global/row-1?source_transcript=row-1#message-exact-opening' : '/product-conversations/pc-1?source_transcript=row-1#message-exact-opening');
    await waitForPageReady();
    expect(screen.getByRole('alert')).toHaveTextContent('linked message is not available');
    expect(currentPositioning().kind).toBe('idle');
  });

  it.each([false, true])('rejects a predecessor boundary hash pinned to the successor without an opening alias (Global=%s)', async global => {
    const { api } = await import('../api');
    const snapshot = makeSnapshot({ before: null, has_older: false, boundaries: [{ message_id: 'cont-1', represented_successor_opening_message_id: 'exact-opening', predecessor_transcript_row_id: 'row-1', successor_transcript_row_id: 'row-2', segment_ordinal: 1 }] });
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValue(coordinatorSnapshot(snapshot));
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValue(snapshot);
    renderPage(global ? '/global/row-2?source_transcript=row-2#message-cont-1' : '/product-conversations/pc-1?source_transcript=row-2#message-cont-1');
    await waitForPageReady();
    expect(screen.getByRole('alert')).toHaveTextContent('linked message is not available');
    expect(currentPositioning().kind).toBe('idle');
  });

  it.each([false, true])('retains an advanced cursor through a full empty-prefix topology roster refresh (Global=%s)', async global => {
    const { api } = await import('../api');
    const complete = makeSnapshot();
    const initial = { ...complete, segments: complete.segments.map(segment => segment.transcript_row_id === 'row-1' ? { ...segment, messages: [] } : segment), before: 'tail-cursor' };
    const acquired = { ...complete, before: 'advanced-cursor', has_older: true };
    const refreshed = { ...initial, before: 'refreshed-tail', updated_at: '2026-01-02T00:00:00Z' };
    const exhausted = { ...complete, before: null, has_older: false };
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValueOnce(coordinatorSnapshot(initial)).mockResolvedValueOnce(coordinatorSnapshot(acquired)).mockResolvedValueOnce(coordinatorSnapshot(refreshed)).mockResolvedValueOnce(coordinatorSnapshot(exhausted));
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(initial).mockResolvedValueOnce(acquired).mockResolvedValueOnce(refreshed).mockResolvedValueOnce(exhausted);
    renderPage(global ? '/global/row-2#message-m-3' : '/product-conversations/pc-1#message-m-3');
    await waitForPageReady();
    fireEvent.click(screen.getByRole('button', { name: 'complete positioning' }));
    fireEvent.click(screen.getByRole('button', { name: 'load older with anchor' }));
    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent('m-1'));
    fireEvent.click(screen.getByRole('button', { name: 'complete positioning' }));
    act(() => window.dispatchEvent(new Event('phoenix:automatic-continuation-updated')));
    await waitFor(() => expect(global ? api.getCoordinatorHistory : api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(3));
    fireEvent.click(screen.getByRole('button', { name: 'load older' }));
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.hasOlderMessages).toBe(false));
    expect(global ? api.getCoordinatorHistory : api.getProductConversationHistorySnapshot).toHaveBeenLastCalledWith(global ? 'row-1' : 'pc-1', expect.objectContaining({ message_limit: 100, before: 'advanced-cursor', open_id: expect.any(String), onTelemetry: expect.any(Function) }));
    expect(screen.getByTestId('message-order').textContent).toBe('m-1,m-2,cont-1,m-3,m-4');
    expect(currentPositioning().kind).toBe('idle');
  });

  it.each([false, true])('retains exhausted same-row sequence coverage when a refreshed tail still advertises older rows (Global=%s)', async global => {
    const { api } = await import('../api');
    const rows = Array.from({ length: 20 }, (_, index) => makeMessage(`covered-${index + 1}`, index + 1, 'row-2'));
    const initial = makeSnapshot({ boundaries: [], segments: [{ segment_ordinal: 0, transcript_row_id: 'row-1', slug: 'row-1', title: 'Empty root', messages: [] }, { segment_ordinal: 1, transcript_row_id: 'row-2', slug: 'row-2', title: 'Latest', messages: rows.slice(10) }] });
    const acquired = { ...initial, before: null, has_older: false, segments: initial.segments.map(segment => segment.transcript_row_id === 'row-2' ? { ...segment, messages: rows.slice(0, 10) } : segment) };
    const refreshed = { ...initial, before: 'fresh-tail' };
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValueOnce(coordinatorSnapshot(initial)).mockResolvedValueOnce(coordinatorSnapshot(acquired)).mockResolvedValueOnce(coordinatorSnapshot(refreshed));
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(initial).mockResolvedValueOnce(acquired).mockResolvedValueOnce(refreshed);
    renderPage(global ? '/global/row-2' : '/product-conversations/pc-1');
    await waitForPageReady();
    fireEvent.click(screen.getByRole('button', { name: 'load older' }));
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.hasOlderMessages).toBe(false));
    act(() => window.dispatchEvent(new Event('phoenix:automatic-continuation-updated')));
    await waitFor(() => expect(global ? api.getCoordinatorHistory : api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(3));
    expect(conversationNavStackSpy.mock.lastCall?.[0]?.hasOlderMessages).toBe(false);
    expect(screen.getByTestId('message-order').textContent).toBe(rows.map(message => message.message_id).join(','));
  });

  it.each([[false, 'continuation'], [true, 'continuation'], [false, 'gap'], [true, 'gap']] as const)('adopts a fresh cursor instead of extrapolating coverage (Global=%s, change=%s)', async (global, change) => {
    const { api } = await import('../api');
    const rows = Array.from({ length: 30 }, (_, index) => makeMessage(`anchor-${index + 1}`, index + 1, 'row-2'));
    const root = { segment_ordinal: 0, transcript_row_id: 'row-1', slug: 'row-1', title: 'Empty root', messages: [] };
    const member = { segment_ordinal: 1, transcript_row_id: 'row-2', slug: 'row-2', title: 'Latest', messages: rows.slice(10, 20) };
    const initial = makeSnapshot({ boundaries: [], segments: [root, member], before: 'old-tail' });
    const acquired = { ...initial, before: 'advanced-old-generation', segments: [root, { ...member, messages: rows.slice(0, 10) }] };
    const refreshed = change === 'continuation'
      ? { ...initial, latest_transcript_row_id: 'row-3', writable_transcript_row_id: 'row-3', before: 'fresh-generation', segments: [root, member, { segment_ordinal: 2, transcript_row_id: 'row-3', slug: 'row-3', title: 'Newest', messages: [makeMessage('new-member', 1, 'row-3')] }] }
      : { ...initial, before: 'fresh-gap', segments: [root, { ...member, messages: rows.slice(25) }] };
    const older = { ...refreshed, before: null, has_older: false, segments: [root, { ...member, messages: rows.slice(20, 25) }] };
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValueOnce(coordinatorSnapshot(initial)).mockResolvedValueOnce(coordinatorSnapshot(acquired)).mockResolvedValueOnce(coordinatorSnapshot(refreshed)).mockResolvedValueOnce(coordinatorSnapshot(older));
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(initial).mockResolvedValueOnce(acquired).mockResolvedValueOnce(refreshed).mockResolvedValueOnce(older);
    renderPage(global ? '/global/row-2#message-anchor-11' : '/product-conversations/pc-1#message-anchor-11');
    await waitForPageReady();
    fireEvent.click(screen.getByRole('button', { name: 'complete positioning' }));
    fireEvent.click(screen.getByRole('button', { name: 'load older' }));
    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent('anchor-1,'));
    act(() => window.dispatchEvent(new Event('phoenix:automatic-continuation-updated')));
    await waitFor(() => expect(global ? api.getCoordinatorHistory : api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(3));
    expect(currentPositioning().kind).toBe('idle');
    act(() => { void (conversationNavStackSpy.mock.lastCall?.[0]?.onLoadOlderMessages as (basis: unknown) => Promise<void>)({ kind: 'reader_anchor', messageId: 'anchor-11', viewportStartOffset: 7 }); });
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.hasOlderMessages).toBe(false));
    expect(global ? api.getCoordinatorHistory : api.getProductConversationHistorySnapshot).toHaveBeenLastCalledWith(global ? 'row-1' : 'pc-1', expect.objectContaining({ message_limit: 100, before: refreshed.before, open_id: expect.any(String), onTelemetry: expect.any(Function) }));
    expect(screen.getByTestId('message-order')).toHaveTextContent('anchor-1,anchor-2');
    expect(currentPositioning()).toMatchObject({ kind: 'positioning', command: { kind: 'restore_after_prefix_expansion', messageId: 'anchor-11', viewportStartOffset: 7 } });
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.slug).toBe(change === 'continuation' ? 'row-3' : 'row-2');
  });

  it.each([false, true])('does not fabricate pending opening authority or hide a failed readiness refresh (Global=%s)', async global => {
    const { api } = await import('../api');
    const pending = makeSnapshot({ latest_opening: pendingOpening(), writable_transcript_row_id: null, before: null, has_older: false });
    pending.segments = pending.segments.map(segment => segment.transcript_row_id === 'row-2' ? { ...segment, messages: [] } : segment);
    const opening = { ...makeMessage('pending-generated-opening', 1, 'row-2'), message_type: 'continuation' as const, content: { type: 'continuation', summary: 'First handoff' } };
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValueOnce(coordinatorSnapshot(pending)).mockRejectedValueOnce(new Error('opening authority refresh failed'));
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(pending).mockRejectedValueOnce(new Error('opening authority refresh failed'));
    renderPage(global ? '/global/row-1' : '/product-conversations/pc-1');
    await waitForPageReady();
    act(() => emitLatestProjection({ messages: [opening], pendingMessages: [], convState: { type: 'idle' } }));
    act(() => notifyProductConversationSnapshotChanged('pc-1'));
    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('opening authority refresh failed'));
    expect(pending.boundaries[0]?.represented_successor_opening_message_id).toBeNull();
    expect(screen.getByTestId('message-order').textContent).toBe('m-1,m-2,cont-1,pending-generated-opening');
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.mutationEnabled).toBe(false);
  });

  it('keeps a completed navigation idle after a subsequent reader-anchor restoration finishes', async () => {
    const { api } = await import('../api');
    const initial = makeSnapshot();
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(initial)
      .mockResolvedValueOnce({ ...initial, before: null, has_older: false });
    renderPage('/product-conversations/pc-1#message-m-3');
    await waitForPageReady();
    fireEvent.click(screen.getByRole('button', { name: 'complete positioning' }));
    expect(currentPositioning().kind).toBe('idle');
    fireEvent.click(screen.getByRole('button', { name: 'load older with anchor' }));
    await waitFor(() => expect(currentPositioning()).toMatchObject({ command: { kind: 'restore_after_prefix_expansion' } }));
    fireEvent.click(screen.getByRole('button', { name: 'complete positioning' }));
    expect(currentPositioning().kind).toBe('idle');
  });

  it('does not apply an old prefix anchor after the reader explicitly navigates to another message', async () => {
    const { api } = await import('../api');
    const initial = makeSnapshot();
    let resolveOlder!: (page: ProductConversationSnapshotView) => void;
    const pending = new Promise<ProductConversationSnapshotView>((resolve) => { resolveOlder = resolve; });
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(initial).mockReturnValueOnce(pending);
    renderPage('/product-conversations/pc-1');
    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent('m-3'));
    fireEvent.click(screen.getByRole('button', { name: 'load older with anchor' }));
    fireEvent.click(screen.getByRole('button', { name: 'jump latest message' }));
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning).toMatchObject({ kind: 'positioning', command: { kind: 'jump_to_message', targetMessageId: 'm-3' } }));
    fireEvent.click(screen.getByRole('button', { name: 'complete positioning' }));
    await act(async () => resolveOlder({ ...initial, before: null, has_older: false }));
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.hasOlderMessages).toBe(false));
    expect(conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning).toMatchObject({ kind: 'idle' });
    expect(screen.getByTestId('history-location')).toHaveTextContent('#message-m-3');
  });

  it('lazily selects an exact historical transcript-only Global pin without moving its composer', async () => {
    const { api } = await import('../api');
    const complete = coordinatorSnapshot(makeSnapshot({ before: null, has_older: false }));
    const tail = { ...complete, segments: complete.segments.map((segment) => segment.transcript_row_id === 'row-1' ? { ...segment, messages: [] } : segment), before: 'global-before', has_older: true };
    vi.mocked(api.getCoordinatorHistory).mockResolvedValueOnce(tail).mockResolvedValueOnce(complete);
    renderPage('/global/row-1?source_transcript=row-1');
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning).toMatchObject({ kind: 'positioning', command: { targetMessageId: 'm-1' } }));
    expect(api.getCoordinatorHistory).toHaveBeenCalledTimes(2);
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.slug).toBe('row-2');
    expect(screen.getByTestId('history-location')).toHaveTextContent('/global/row-1?source_transcript=row-1');
  });

  it('repositions an acknowledged message link when Back returns from a hashless entry', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getCoordinatorHistory).mockResolvedValue(coordinatorSnapshot(makeSnapshot({ before: null, has_older: false })));
    renderPage('/global/row-1?source_transcript=row-1#message-m-1');
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning).toMatchObject({ kind: 'positioning', command: { targetMessageId: 'm-1' } }));
    fireEvent.click(screen.getByRole('button', { name: 'complete positioning' }));
    fireEvent.click(screen.getByRole('button', { name: 'clear message hash' }));
    fireEvent.click(screen.getByRole('button', { name: 'history back' }));
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning).toMatchObject({ kind: 'positioning', command: { targetMessageId: 'm-1' } }));
    expect(api.getCoordinatorHistory).toHaveBeenCalledTimes(1);
  });

  it.each([
    ['ordinary', 'reader_anchor'], ['ordinary', 'reader_viewport'],
    ['coordinator', 'reader_anchor'], ['coordinator', 'reader_viewport'],
  ] as const)('keeps newer same-route %s %s intent when an older page completes', async (kind, basisKind) => {
    const { api } = await import('../api');
    const initial = makeSnapshot();
    let resolveOlder!: (page: ProductConversationSnapshotView) => void;
    const pending = new Promise<ProductConversationSnapshotView>(resolve => { resolveOlder = resolve; });
    if (kind === 'ordinary') {
      vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(initial).mockReturnValueOnce(pending);
    } else {
      vi.mocked(api.getCoordinatorHistory).mockResolvedValueOnce({ ...coordinatorSnapshot(initial), requested_transcript_row_id: initial.latest_transcript_row_id }).mockImplementationOnce(async () => {
        const page = await pending;
        return { ...coordinatorSnapshot(page), requested_transcript_row_id: page.latest_transcript_row_id };
      });
    }
    renderPage(kind === 'coordinator' ? `/global/${initial.latest_transcript_row_id}` : '/product-conversations/pc-1');
    await waitForPageReady();
    fireEvent.click(screen.getByRole('button', { name: 'load older with anchor' }));
    const update = conversationNavStackSpy.mock.lastCall?.[0]?.onUpdateOlderMessagesRestore as ((basis: import('../conversation/historyExpansion').RestoreBasis) => void) | undefined;
    expect(update).toBeDefined();
    act(() => update?.(basisKind === 'reader_anchor'
      ? { kind: 'reader_anchor', messageId: 'm-4', viewportStartOffset: 35 }
      : { kind: 'reader_viewport' }));
    await act(async () => resolveOlder(makeSnapshot({ before: null, has_older: false,
      segments: [{ segment_ordinal: 0, transcript_row_id: 'older', slug: 'older', title: 'Older', messages: [makeMessage('old-message', 1, 'older')], handoff: null }],
    })));
    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent('old-message'));
    const positioning = conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning;
    if (basisKind === 'reader_anchor') expect(positioning).toMatchObject({ kind: 'positioning', command: { kind: 'restore_after_prefix_expansion', messageId: 'm-4', viewportStartOffset: 35 } });
    else expect(positioning).toMatchObject({ kind: 'idle' });
    expect(screen.getByTestId('history-location')).not.toHaveTextContent('#');
  });

  it('retires an implicit historical Global route anchor when selecting a local pending chapter', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getCoordinatorHistory).mockResolvedValue(coordinatorSnapshot(makeSnapshot({ before: null, has_older: false })));
    renderPage('/global/row-1?source_transcript=row-1&source_tool=old-call#message-m-1');
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning).toMatchObject({ command: { targetMessageId: 'm-1' } }));
    fireEvent.click(screen.getByRole('button', { name: 'select pending chapter' }));
    expect(conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning).toMatchObject({ kind: 'idle' });
    expect(screen.getByTestId('history-location')).toHaveTextContent('/global/row-1');
    expect(screen.getByTestId('history-location')).not.toHaveTextContent('source_transcript');
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.slug).toBe('row-2');
    fireEvent.click(screen.getByRole('button', { name: 'history back' }));
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning).toMatchObject({ command: { targetMessageId: 'm-1' } }));
  });

  it.each([[false, false], [true, false], [false, true], [true, true]])('positions an omitted generated-opening bookmark at its exact persisted boundary (Global=%s, lazy=%s)', async (global, lazy) => {
    const { api } = await import('../api');
    const initial = makeSnapshot({ before: null, has_older: false, boundaries: [{
      message_id: 'cont-1', represented_successor_opening_message_id: 'represented-opening', predecessor_transcript_row_id: 'row-1', successor_transcript_row_id: 'row-2', segment_ordinal: 0,
    }] });
    const tail = lazy ? { ...initial, before: 'alias-older', has_older: true, segments: initial.segments.map(segment => ({ ...segment, messages: segment.messages.filter(message => message.message_id !== 'cont-1') })) } : initial;
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValue(coordinatorSnapshot(initial)).mockResolvedValueOnce(coordinatorSnapshot(tail));
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValue(initial).mockResolvedValueOnce(tail);
    renderPage(global ? '/global/row-2?source_transcript=row-2#message-represented-opening' : '/product-conversations/pc-1?source_transcript=row-2#message-represented-opening');
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning).toMatchObject({ command: { targetMessageId: 'cont-1' } }));
    expect(screen.queryByText('The linked message is not available')).not.toBeInTheDocument();
    expect(screen.getByTestId('history-location')).toHaveTextContent('#message-represented-opening');
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.slug).toBe('row-2');
    expect(global ? api.getCoordinatorHistory : api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(lazy ? 2 : 1);
  });

  it.each([[false, false], [true, false], [false, true], [true, true]])('positions a hashless opening-only source pin at its exact represented boundary (Global=%s, lazy=%s)', async (global, lazy) => {
    const { api } = await import('../api');
    const opening = { ...makeMessage('represented-opening', 1, 'row-2'), message_type: 'continuation' as const, content: { type: 'continuation', summary: 'First handoff' } };
    const complete = makeSnapshot({ before: null, has_older: false, boundaries: [{
      message_id: 'cont-1', represented_successor_opening_message_id: opening.message_id, predecessor_transcript_row_id: 'row-1', successor_transcript_row_id: 'row-2', segment_ordinal: 1,
    }] });
    complete.segments = complete.segments.map(segment => segment.transcript_row_id === 'row-2' ? { ...segment, messages: [opening] } : segment);
    const tail = { ...complete, before: 'opening-before', has_older: true, segments: complete.segments.map(segment => segment.transcript_row_id === 'row-1' ? { ...segment, messages: [makeMessage('m-2', 2, 'row-1')] } : segment) };
    const intermediate = { ...tail, before: 'boundary-before', segments: tail.segments.map(segment => segment.transcript_row_id === 'row-1' ? { ...segment, messages: [makeMessage('m-1', 1, 'row-1')] } : segment) };
    if (global) {
      vi.mocked(api.getCoordinatorHistory).mockResolvedValue(coordinatorSnapshot(complete));
      if (lazy) vi.mocked(api.getCoordinatorHistory).mockResolvedValueOnce(coordinatorSnapshot(tail)).mockResolvedValueOnce(coordinatorSnapshot(intermediate));
    } else {
      vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValue(complete);
      if (lazy) vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(tail).mockResolvedValueOnce(intermediate);
    }
    renderPage(global ? '/global/row-2?source_transcript=row-2&keep=original' : '/product-conversations/pc-1?source_transcript=row-2&keep=original');
    await waitFor(() => expect(currentPositioning()).toMatchObject({ kind: 'positioning', command: { targetMessageId: 'cont-1' } }));
    expect(screen.queryByText('Original source transcript unavailable.')).not.toBeInTheDocument();
    expect(screen.getByTestId('history-location').textContent).toBe(global ? '/global/row-2?source_transcript=row-2&keep=original' : '/product-conversations/pc-1?source_transcript=row-2&keep=original');
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]).toMatchObject({ slug: 'row-2', mutationEnabled: true });
    expect((conversationNavStackSpy.mock.lastCall?.[0]?.messages as Message[]).map(message => message.message_id)).toEqual(['m-1', 'm-2', 'cont-1']);
    expect(conversationNavStackSpy.mock.calls.some(([props]) => props.transcriptPositioning.kind === 'positioning' && props.transcriptPositioning.command.targetMessageId !== 'cont-1')).toBe(false);
    const loader = global ? api.getCoordinatorHistory : api.getProductConversationHistorySnapshot;
    expect(loader).toHaveBeenCalledTimes(lazy ? 3 : 1);
    if (lazy) {
      expect(loader).toHaveBeenNthCalledWith(2, global ? 'row-1' : 'pc-1', expect.objectContaining({ message_limit: 100, before: 'opening-before', open_id: expect.any(String), onTelemetry: expect.any(Function) }));
      expect(loader).toHaveBeenNthCalledWith(3, global ? 'row-1' : 'pc-1', expect.objectContaining({ message_limit: 100, before: 'boundary-before', open_id: expect.any(String), onTelemetry: expect.any(Function) }));
    }
  });

  it.each([[false, false], [true, false], [false, true], [true, true]])('does not invent a boundary for an empty unrepresented source pin (Global=%s, pending=%s)', async (global, pending) => {
    const { api } = await import('../api');
    const initial = makeSnapshot({ before: null, has_older: false, latest_opening: pending ? pendingOpening() : { status: 'none' }, writable_transcript_row_id: pending ? null : 'row-2' });
    initial.segments = initial.segments.map(segment => segment.transcript_row_id === 'row-2' ? { ...segment, messages: [] } : segment);
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValue(coordinatorSnapshot(initial));
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValue(initial);
    renderPage(global ? '/global/row-2?source_transcript=row-2' : '/product-conversations/pc-1?source_transcript=row-2');
    await waitFor(() => expect(screen.getByText('Original source transcript unavailable.')).toBeInTheDocument());
    expect(currentPositioning()).toMatchObject({ kind: 'idle' });
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]).toMatchObject({ slug: 'row-2', mutationEnabled: !pending });
    expect(global ? api.getCoordinatorHistory : api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(1);
  });

  it.each([false, true])('selects an explicit equal-text successor instruction instead of its represented opening (Global=%s)', async (global) => {
    const { api } = await import('../api');
    const instruction = { ...makeMessage('user-instruction', 2, 'row-2'), content: { type: 'user', text: 'First handoff' } };
    const initial = makeSnapshot({ before: null, has_older: false, boundaries: [{
      message_id: 'cont-1', represented_successor_opening_message_id: 'represented-opening', predecessor_transcript_row_id: 'row-1', successor_transcript_row_id: 'row-2', segment_ordinal: 1,
    }] });
    initial.segments = initial.segments.map(segment => segment.transcript_row_id === 'row-2' ? { ...segment, messages: [instruction] } : segment);
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValue(coordinatorSnapshot(initial));
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValue(initial);
    renderPage(global ? '/global/row-2?source_transcript=row-2#message-user-instruction' : '/product-conversations/pc-1?source_transcript=row-2#message-user-instruction');
    await waitFor(() => expect(currentPositioning()).toMatchObject({ command: { targetMessageId: instruction.message_id } }));
    expect((conversationNavStackSpy.mock.lastCall?.[0]?.messages as Message[]).find(message => message.message_id === instruction.message_id)?.content).toEqual(instruction.content);
    fireEvent.click(screen.getByRole('button', { name: 'clear message hash' }));
    await waitFor(() => expect(currentPositioning()).toMatchObject({ command: { targetMessageId: instruction.message_id } }));
    expect(screen.queryByText('Original source transcript unavailable.')).not.toBeInTheDocument();
  });

  it.each([false, true])('excludes only the exact represented successor opening from live hydration (Global=%s)', async (global) => {
    const { api } = await import('../api');
    const boundary = { ...makeMessage('original-boundary', 2, 'row-1'), message_type: 'continuation' as const, content: { type: 'continuation', summary: 'Persisted summary' } };
    const opening = { ...makeMessage('represented-opening', 1, 'row-2'), message_type: 'continuation' as const, content: { type: 'continuation', summary: 'Persisted summary' } };
    const editedOpening = { ...makeMessage('distinct-opening', 2, 'row-2'), content: { type: 'user', text: 'Persisted summary plus explicit instruction' } };
    const sameTextDifferentIdentity = { ...makeMessage('distinct-repeat', 3, 'row-2'), content: { type: 'user', text: 'Persisted summary' } };
    const initial = makeSnapshot({ before: null, has_older: false, segments: [
      { segment_ordinal: 0, transcript_row_id: 'row-1', slug: 'row-1', title: 'Old', messages: [boundary] },
      { segment_ordinal: 1, transcript_row_id: 'row-2', slug: 'row-2', title: 'Newest', messages: [] },
    ], boundaries: [{ represented_successor_opening_message_id: 'represented-opening', message_id: 'original-boundary', predecessor_transcript_row_id: 'row-1', successor_transcript_row_id: 'row-2', segment_ordinal: 0 }] });
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValue(coordinatorSnapshot(initial));
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValue(initial);
    renderPage(global ? '/global/row-2' : '/product-conversations/pc-1');
    await waitForPageReady();
    act(() => emitLatestProjection({ messages: [opening, editedOpening, sameTextDifferentIdentity], pendingMessages: [], convState: { type: 'idle' } }));
    await waitFor(() => expect((conversationNavStackSpy.mock.lastCall?.[0]?.messages as Message[]).map(message => message.message_id)).toEqual(['original-boundary', 'distinct-opening', 'distinct-repeat']));
    const continued = makeSnapshot({ ...initial, latest_transcript_row_id: 'row-3', writable_transcript_row_id: 'row-3', segments: [...initial.segments, {
      segment_ordinal: 2, transcript_row_id: 'row-3', slug: 'row-3', title: 'Newest', messages: [makeMessage('newest-message', 1, 'row-3')],
    }] });
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValue(coordinatorSnapshot(continued));
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValue(continued);
    act(() => window.dispatchEvent(new Event('phoenix:automatic-continuation-updated')));
    await waitFor(() => expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.slug).toBe('row-3'));
    expect((conversationNavStackSpy.mock.lastCall?.[0]?.messages as Message[]).map(message => message.message_id)).toEqual(['original-boundary', 'distinct-opening', 'distinct-repeat', 'newest-message']);
  });

  it('positions the new hashless Global route member without replacing the acquired aggregate owner', async () => {
    const { api } = await import('../api');
    const initial = makeSnapshot({ latest_transcript_row_id: 'row-3', writable_transcript_row_id: 'row-3', before: null, has_older: false });
    initial.segments.push({ segment_ordinal: 2, transcript_row_id: 'row-3', slug: 'row-3', title: 'Newest', messages: [makeMessage('newest-message', 1, 'row-3')] });
    vi.mocked(api.getCoordinatorHistory).mockResolvedValue(coordinatorSnapshot(initial));
    renderPage('/global/row-1', true);
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning).toMatchObject({ command: { targetMessageId: 'm-1' } }));
    fireEvent.click(screen.getByRole('button', { name: 'complete positioning' }));
    fireEvent.click(screen.getByRole('button', { name: 'open second Global member' }));
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning).toMatchObject({ command: { targetMessageId: 'm-3' } }));
    expect(api.getCoordinatorHistory).toHaveBeenCalledTimes(1);
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.slug).toBe('row-3');
    fireEvent.click(screen.getByRole('button', { name: 'history back' }));
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning).toMatchObject({ command: { targetMessageId: 'm-1' } }));
  });

  it('refreshes unresolved warm Global route topology without substituting its old requested member', async () => {
    const { api } = await import('../api');
    const initial = makeSnapshot({ before: null, has_older: false });
    let release!: (snapshot: CoordinatorConversationSnapshotView) => void;
    vi.mocked(api.getCoordinatorHistory).mockResolvedValueOnce(coordinatorSnapshot(initial)).mockReturnValueOnce(new Promise(resolve => { release = resolve; }));
    renderPage('/global/row-1', true);
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning).toMatchObject({ command: { targetMessageId: 'm-1' } }));
    fireEvent.click(screen.getByRole('button', { name: 'complete positioning' }));
    fireEvent.click(screen.getByRole('button', { name: 'open newly created Global member' }));
    await waitFor(() => expect(api.getCoordinatorHistory).toHaveBeenCalledTimes(2));
    expect(conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning).toMatchObject({ kind: 'idle' });
    const continued = { ...initial, latest_transcript_row_id: 'row-5', writable_transcript_row_id: 'row-5', segments: [...initial.segments,
      { segment_ordinal: 2, transcript_row_id: 'row-4', slug: 'row-4', title: 'Requested', messages: [makeMessage('requested-new-member', 1, 'row-4')] },
      { segment_ordinal: 3, transcript_row_id: 'row-5', slug: 'row-5', title: 'Newest', messages: [makeMessage('newest-member', 1, 'row-5')] },
    ] };
    await act(async () => release(coordinatorSnapshot(continued)));
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning).toMatchObject({ command: { targetMessageId: 'requested-new-member' } }));
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.slug).toBe('row-5');
  });

  it('keeps a hashless newest Global reader unpinned when its route member continues', async () => {
    const { api } = await import('../api');
    const initial = { ...coordinatorSnapshot(makeSnapshot({ before: null, has_older: false })), requested_transcript_row_id: 'row-2' };
    const continued = { ...initial, latest_transcript_row_id: 'row-3', writable_transcript_row_id: 'row-3', segments: [...initial.segments, {
      segment_ordinal: 2, transcript_row_id: 'row-3', slug: 'row-3', title: 'Newest', messages: [makeMessage('newest-message', 1, 'row-3')],
    }] };
    vi.mocked(api.getCoordinatorHistory).mockResolvedValueOnce(initial).mockResolvedValueOnce(continued);
    renderPage('/global/row-2');
    await waitFor(() => expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.slug).toBe('row-2'));
    expect(conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning).toMatchObject({ kind: 'idle' });
    act(() => window.dispatchEvent(new Event('phoenix:automatic-continuation-updated')));
    await waitFor(() => expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.slug).toBe('row-3'));
    expect(conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning).toMatchObject({ kind: 'idle' });
    expect(screen.getByTestId('history-location')).toHaveTextContent('/global/row-2');
  });

  it.each([false, true])('retains acquired live predecessor rows across a bounded continuation refresh (Global=%s)', async (global) => {
    const { api } = await import('../api');
    const initial = makeSnapshot({ before: null, has_older: false, segments: [{
      segment_ordinal: 0, transcript_row_id: 'row-2', slug: 'row-2', title: 'Predecessor',
      messages: Array.from({ length: 100 }, (_, index) => makeMessage(`acquired-${index + 1}`, index + 1, 'row-2')),
    }] });
    const live = Array.from({ length: 250 }, (_, index) => makeMessage(`acquired-${index + 1}`, index + 1, 'row-2'));
    const continued = makeSnapshot({ latest_transcript_row_id: 'row-3', writable_transcript_row_id: 'row-3', before: 'bounded-tail', has_older: true, segments: [
      { ...initial.segments[0]!, messages: live.slice(151) },
      { segment_ordinal: 1, transcript_row_id: 'row-3', slug: 'row-3', title: 'Newest', messages: [makeMessage('newest-message', 1, 'row-3')] },
    ] });
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValue({ ...coordinatorSnapshot(continued), requested_transcript_row_id: 'row-2' }).mockResolvedValueOnce({ ...coordinatorSnapshot(initial), requested_transcript_row_id: 'row-2' });
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValue(continued).mockResolvedValueOnce(initial);
    renderPage(global ? '/global/row-2#message-acquired-125' : '/product-conversations/pc-1#message-acquired-125');
    await waitForPageReady();
    act(() => emitLatestProjection({ messages: live, pendingMessages: [], convState: { type: 'idle' } }));
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning).toMatchObject({ command: { targetMessageId: 'acquired-125' } }));
    fireEvent.click(screen.getByRole('button', { name: 'complete positioning' }));
    act(() => window.dispatchEvent(new Event('phoenix:automatic-continuation-updated')));
    await waitFor(() => expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.slug).toBe('row-3'));
    const props = conversationNavStackSpy.mock.lastCall?.[0];
    expect((props?.messages as Message[]).map(message => message.message_id)).toEqual([...live.map(message => message.message_id), 'newest-message']);
    expect(props?.transcriptPositioning).toMatchObject({ kind: 'idle' });
    expect(screen.getByTestId('history-location')).toHaveTextContent('#message-acquired-125');
  });

  it.each([false, true])('orders a paged predecessor gap between retained prefix and refreshed tail (Global=%s)', async (global) => {
    const { api } = await import('../api');
    const rows = Array.from({ length: 250 }, (_, index) => makeMessage(`gap-${index + 1}`, index + 1, 'row-2'));
    const segment = { segment_ordinal: 0, transcript_row_id: 'row-2', slug: 'row-2', title: 'Predecessor', messages: rows.slice(0, 100) };
    const initial = makeSnapshot({ before: null, has_older: false, segments: [segment] });
    const continued = makeSnapshot({ latest_transcript_row_id: 'row-3', writable_transcript_row_id: 'row-3', before: 'gap-cursor', has_older: true, segments: [
      { ...segment, messages: rows.slice(151) },
      { segment_ordinal: 1, transcript_row_id: 'row-3', slug: 'row-3', title: 'Newest', messages: [makeMessage('newest-message', 1, 'row-3')] },
    ] });
    const gap = { ...continued, before: null, has_older: false, segments: [{ ...segment, messages: rows.slice(51, 151) }] };
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValueOnce({ ...coordinatorSnapshot(initial), requested_transcript_row_id: 'row-2' }).mockResolvedValueOnce({ ...coordinatorSnapshot(continued), requested_transcript_row_id: 'row-2' }).mockResolvedValueOnce({ ...coordinatorSnapshot(gap), requested_transcript_row_id: 'row-2' });
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(initial).mockResolvedValueOnce(continued).mockResolvedValueOnce(gap);
    renderPage(global ? '/global/row-2' : '/product-conversations/pc-1');
    await waitForPageReady();
    act(() => window.dispatchEvent(new Event('phoenix:automatic-continuation-updated')));
    await waitFor(() => expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.slug).toBe('row-3'));
    fireEvent.click(screen.getByRole('button', { name: 'load older' }));
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.hasOlderMessages).toBe(false));
    expect((conversationNavStackSpy.mock.lastCall?.[0]?.messages as Message[]).map(message => message.message_id)).toEqual([...rows.map(message => message.message_id), 'newest-message']);
  });

  it.each([false, true])('retains the fresh cursor when a delayed old-ceiling page finishes across continuation (Global=%s)', async global => {
    const { api } = await import('../api');
    const rows = Array.from({ length: 400 }, (_, index) => makeMessage(`ceiling-${index + 1}`, index + 1, 'row-2'));
    const segment = { segment_ordinal: 0, transcript_row_id: 'row-2', slug: 'row-2', title: 'Predecessor', messages: rows.slice(100, 200) };
    const initial = makeSnapshot({ before: 'old-ceiling', has_older: true, segments: [segment] });
    const oldPrefix = { ...initial, before: null, has_older: false, segments: [{ ...segment, messages: rows.slice(0, 100) }] };
    const fresh = makeSnapshot({ latest_transcript_row_id: 'row-3', writable_transcript_row_id: 'row-3', before: 'fresh-ceiling', has_older: true, segments: [
      { ...segment, messages: rows.slice(301) },
      { segment_ordinal: 1, transcript_row_id: 'row-3', slug: 'row-3', title: 'Newest', messages: [makeMessage('newest-message', 1, 'row-3')] },
    ] });
    let release!: () => void;
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValueOnce(coordinatorSnapshot(initial)).mockReturnValueOnce(new Promise(resolve => { release = () => resolve(coordinatorSnapshot(oldPrefix)); })).mockResolvedValueOnce(coordinatorSnapshot(fresh)).mockResolvedValueOnce(coordinatorSnapshot({ ...fresh, before: null, has_older: false, segments: [{ ...segment, messages: rows.slice(100, 301) }] }));
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(initial).mockReturnValueOnce(new Promise(resolve => { release = () => resolve(oldPrefix); })).mockResolvedValueOnce(fresh).mockResolvedValueOnce({ ...fresh, before: null, has_older: false, segments: [{ ...segment, messages: rows.slice(100, 301) }] });
    renderPage(global ? '/global/row-2' : '/product-conversations/pc-1');
    await waitForPageReady();
    fireEvent.click(screen.getByRole('button', { name: 'load older' }));
    act(() => window.dispatchEvent(new Event('phoenix:automatic-continuation-updated')));
    await waitFor(() => expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.slug).toBe('row-3'));
    await act(async () => release());
    expect(conversationNavStackSpy.mock.lastCall?.[0]?.hasOlderMessages).toBe(true);
    fireEvent.click(screen.getByRole('button', { name: 'load older' }));
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.hasOlderMessages).toBe(false));
    expect((global ? api.getCoordinatorHistory : api.getProductConversationHistorySnapshot)).toHaveBeenLastCalledWith(expect.any(String), expect.objectContaining({ before: 'fresh-ceiling', message_limit: 100, open_id: expect.any(String), onTelemetry: expect.any(Function) }));
    expect((conversationNavStackSpy.mock.lastCall?.[0]?.messages as Message[]).map(message => message.message_id)).toEqual([...rows.map(message => message.message_id), 'newest-message']);
  });

  it('preserves the aggregate reader anchor and newest composer when continuation races an older page', async () => {
    const { api } = await import('../api');
    const initial = coordinatorSnapshot(makeSnapshot());
    let resolveOlder!: (page: ReturnType<typeof coordinatorSnapshot>) => void;
    const pendingOlder = new Promise<ReturnType<typeof coordinatorSnapshot>>((resolve) => { resolveOlder = resolve; });
    const continued = { ...initial, latest_transcript_row_id: 'row-3', writable_transcript_row_id: 'row-3', segments: [...initial.segments, {
      segment_ordinal: 3, transcript_row_id: 'row-3', slug: 'row-3', title: 'Newest', messages: [makeMessage('newest-message', 1, 'row-3')], handoff: null,
    }] };
    vi.mocked(api.getCoordinatorHistory).mockResolvedValueOnce(initial).mockReturnValueOnce(pendingOlder).mockResolvedValueOnce(continued);
    renderPage('/global/row-1?source_transcript=row-1');
    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent('m-1'));
    fireEvent.click(screen.getByRole('button', { name: 'load older with anchor' }));
    act(() => window.dispatchEvent(new Event('phoenix:automatic-continuation-updated')));
    await waitFor(() => expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.slug).toBe('row-3'));
    await act(async () => resolveOlder({ ...initial, before: null, has_older: false, segments: [{ segment_ordinal: 0, transcript_row_id: 'older', slug: 'older', title: 'Older', messages: [makeMessage('old-message', 1, 'older')] }, ...initial.segments] }));
    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent('old-message'));
    const props = conversationNavStackSpy.mock.lastCall?.[0];
    expect(props?.conversationId).toBe('pc-1');
    expect(props?.transcriptPositioning).toMatchObject({ kind: 'positioning', command: { kind: 'restore_after_prefix_expansion', messageId: 'm-3', viewportStartOffset: 17, view: { conversationId: 'pc-1' } } });
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.slug).toBe('row-3');
    expect((props?.messages as Message[]).map((message) => message.message_id)).toEqual(['old-message', 'm-1', 'm-2', 'cont-1', 'm-3', 'm-4', 'newest-message']);
    expect(screen.getByTestId('history-location')).toHaveTextContent('/global/row-1?source_transcript=row-1');
  });

  it('reissues acknowledged exact-message positioning on browser back/forward without refetching history', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getCoordinatorHistory).mockResolvedValue(coordinatorSnapshot(makeSnapshot({ before: null, has_older: false })));
    renderPage('/global/row-1?source_transcript=row-1#message-m-1');
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning).toMatchObject({ kind: 'positioning', command: { targetMessageId: 'm-1' } }));
    const first = (conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning as { command: { token: number } }).command.token;
    fireEvent.click(screen.getByRole('button', { name: 'complete positioning' }));
    fireEvent.click(screen.getByRole('button', { name: 'jump latest message' }));
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning).toMatchObject({ kind: 'positioning', command: { targetMessageId: 'm-3' } }));
    fireEvent.click(screen.getByRole('button', { name: 'complete positioning' }));
    fireEvent.click(screen.getByRole('button', { name: 'history back' }));
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning).toMatchObject({ kind: 'positioning', command: { targetMessageId: 'm-1' } }));
    expect((conversationNavStackSpy.mock.lastCall?.[0]?.transcriptPositioning as { command: { token: number } }).command.token).toBeGreaterThan(first);
    expect(api.getCoordinatorHistory).toHaveBeenCalledTimes(1);
  });

  beforeEach(async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot).mockReset();
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValue(makeSnapshot());
    vi.mocked(api.getProductConversationAutomaticContinuation).mockReset();
    vi.mocked(api.getProductConversationAutomaticContinuation).mockResolvedValue({
      aggregate: { kind: 'ordinary', product_conversation_id: 'pc-1' },
      auto_continue_on_context_exhaustion: false,
      admission: null,
    });
    vi.mocked(api.updateProductConversationAutomaticContinuation).mockReset();
    vi.mocked(api.reportProductConversationOpen).mockClear();
    vi.mocked(api.getChain).mockReset();
    vi.mocked(api.getChain).mockResolvedValue(makeChain());
    vi.mocked(api.submitChainQuestion).mockReset();
    vi.mocked(api.submitChainQuestion).mockResolvedValue({ chain_qa_id: 'qa-new' });
    vi.mocked(api.getPrStatus).mockReset();
    vi.mocked(api.getPrStatus).mockResolvedValue({
      found: false,
      refresh: { state: 'fresh', stale: false, last_attempted_at: '2026-01-01T00:00:00Z' },
      work_change: { kind: 'clean' },
    });
  });

  it('shows the aggregate automatic-continuation control on an ordinary ProductConversation', async () => {
    const { api } = await import('../api');
    renderPage();

    const control = await screen.findByTestId('automatic-continuation-control');
    await waitFor(() => expect(control).toHaveTextContent('Auto-continue Off'));
    expect(api.getProductConversationAutomaticContinuation).toHaveBeenCalledWith('pc-1');
  });

  it('reports hidden-at-start opens without waiting for or fabricating paint', async () => {
    const { api } = await import('../api');
    const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('hidden');
    renderPage('/product-conversations/pc-hidden');
    await waitForPageReady();
    await waitFor(() => expect(api.reportProductConversationOpen).toHaveBeenCalledTimes(1));
    expect(api.reportProductConversationOpen).toHaveBeenCalledWith(expect.objectContaining({
      first_paint_ms: null,
      visible: false,
    }));
    visibility.mockRestore();
  });

  it('ignores a stale initial snapshot and open report after a route switch', async () => {
    const { api } = await import('../api');
    let resolveFirst: ((snapshot: ProductConversationSnapshotView) => void) | undefined;
    vi.mocked(api.getProductConversationHistorySnapshot)
      .mockImplementationOnce(() => new Promise((resolve) => { resolveFirst = resolve; }))
      .mockResolvedValueOnce(makeSnapshot({
        product_conversation_id: 'pc-2',
        canonical_route: '/product-conversations/pc-2',
        presentation: { kind: 'state', display_name: 'Product Beta', presentation_mode: 'idle' },
      }));
    renderPage('/product-conversations/pc-1', true);
    await waitFor(() => expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(1));
    fireEvent.click(screen.getByRole('button', { name: 'open second product' }));
    expect(await screen.findByRole('heading', { name: 'Product Beta' })).toBeInTheDocument();

    await waitFor(() => expect(api.reportProductConversationOpen).toHaveBeenCalledTimes(1));
    act(() => resolveFirst?.(makeSnapshot({ presentation: { kind: 'state', display_name: 'Stale Alpha', presentation_mode: 'idle' } })));
    await Promise.resolve();
    expect(screen.queryByRole('heading', { name: 'Stale Alpha' })).not.toBeInTheDocument();
    expect(api.reportProductConversationOpen).toHaveBeenCalledTimes(1);
  });

  it('refreshes the active title from the aggregate snapshot invalidation authority', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot)
      .mockResolvedValueOnce(makeSnapshot())
      .mockResolvedValueOnce(makeSnapshot({
        presentation: { kind: 'state', display_name: 'Renamed Product', presentation_mode: 'idle' },
      }));
    renderPage();
    expect(await screen.findByRole('heading', { name: 'Product Alpha' })).toBeInTheDocument();

    act(() => notifyProductConversationSnapshotChanged('pc-1'));

    expect(await screen.findByRole('heading', { name: 'Renamed Product' })).toBeInTheDocument();
    expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(2);
  });

  it('clears a writable open snapshot when its aggregate is deleted', async () => {
    renderPage();
    expect(await screen.findByTestId('product-conversation-composer')).toBeInTheDocument();

    act(() => notifyProductConversationDeleted('pc-1', ['row-1', 'row-2']));

    expect(await screen.findByRole('alert')).toHaveTextContent('This product conversation was deleted.');
    expect(screen.queryByTestId('product-conversation-composer')).not.toBeInTheDocument();
    expect(embeddedConversationPageSpy).toHaveBeenLastCalledWith(expect.objectContaining({
      mutationEnabled: false,
      aggregateLifecycleOpen: false,
    }));
  });

  it('clears an alias-routed writable snapshot when a deleted member matches', async () => {
    renderPage('/product-conversations/root-alias');
    expect(await screen.findByTestId('product-conversation-composer')).toBeInTheDocument();

    act(() => notifyProductConversationDeleted('pc-other', ['row-2']));

    expect(await screen.findByRole('alert')).toHaveTextContent('This product conversation was deleted.');
    expect(screen.queryByTestId('product-conversation-composer')).not.toBeInTheDocument();
  });

  it('defers authoritative absence for an unresolved legacy alias until canonical snapshot', async () => {
    const { api } = await import('../api');
    let resolveSnapshot: ((snapshot: ProductConversationSnapshotView) => void) | undefined;
    vi.mocked(api.getProductConversationHistorySnapshot).mockImplementationOnce(
      () => new Promise((resolve) => { resolveSnapshot = resolve; }),
    );
    renderPage('/product-conversations/root-alias');
    await waitFor(() => expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledOnce());

    act(() => notifyProductConversationsReconciled(new Set(['pc-1'])));
    expect(screen.queryByText('This product conversation was deleted.')).not.toBeInTheDocument();

    act(() => resolveSnapshot?.(makeSnapshot()));
    expect(await screen.findByTestId('product-conversation-composer')).toBeInTheDocument();
    expect(screen.queryByText('This product conversation was deleted.')).not.toBeInTheDocument();
  });

  it('clears a writable snapshot when stream reconciliation is confirmed by authoritative absence', async () => {
    const { api, ApiResponseError } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot)
      .mockResolvedValueOnce(makeSnapshot())
      .mockRejectedValueOnce(new ApiResponseError('not found', 404));
    renderPage();
    expect(await screen.findByTestId('product-conversation-composer')).toBeInTheDocument();

    act(() => notifyProductConversationsReconciled(new Set(['pc-other'])));

    expect(await screen.findByRole('alert')).toHaveTextContent('This product conversation was deleted.');
    expect(screen.queryByTestId('product-conversation-composer')).not.toBeInTheDocument();
  });

  it('refreshes an alias route from canonical aggregate invalidation', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot)
      .mockResolvedValueOnce(makeSnapshot())
      .mockResolvedValueOnce(makeSnapshot({
        presentation: { kind: 'state', display_name: 'Canonical Rename', presentation_mode: 'idle' },
      }));
    renderPage('/product-conversations/root-alias');
    expect(await screen.findByRole('heading', { name: 'Product Alpha' })).toBeInTheDocument();

    act(() => notifyProductConversationSnapshotChanged('pc-1'));

    expect(await screen.findByRole('heading', { name: 'Canonical Rename' })).toBeInTheDocument();
    expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(2);
  });

  it('replays a canonical rename invalidation received while an alias snapshot is loading', async () => {
    const { api } = await import('../api');
    let resolveInitial: ((snapshot: ProductConversationSnapshotView) => void) | undefined;
    vi.mocked(api.getProductConversationHistorySnapshot)
      .mockImplementationOnce(() => new Promise((resolve) => { resolveInitial = resolve; }))
      .mockResolvedValueOnce(makeSnapshot({
        presentation: { kind: 'state', display_name: 'Canonical Rename', presentation_mode: 'idle' },
      }));
    renderPage('/product-conversations/root-alias');
    await waitFor(() => expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(1));

    act(() => notifyProductConversationSnapshotChanged('pc-1'));
    act(() => resolveInitial?.(makeSnapshot()));

    expect(await screen.findByRole('heading', { name: 'Canonical Rename' })).toBeInTheDocument();
    expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(2);
  });

  it('rearms canonical snapshot invalidation after each refresh', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot)
      .mockResolvedValueOnce(makeSnapshot())
      .mockResolvedValueOnce(makeSnapshot({
        presentation: { kind: 'state', display_name: 'First Rename', presentation_mode: 'idle' },
      }))
      .mockResolvedValueOnce(makeSnapshot({
        presentation: { kind: 'state', display_name: 'Second Rename', presentation_mode: 'idle' },
      }));
    renderPage('/product-conversations/root-alias');
    expect(await screen.findByRole('heading', { name: 'Product Alpha' })).toBeInTheDocument();

    act(() => notifyProductConversationSnapshotChanged('pc-1'));
    expect(await screen.findByRole('heading', { name: 'First Rename' })).toBeInTheDocument();
    act(() => notifyProductConversationSnapshotChanged('pc-1'));

    expect(await screen.findByRole('heading', { name: 'Second Rename' })).toBeInTheDocument();
    expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(3);
  });

  it('starts a new measured open when revisiting a previously loaded product route', async () => {
    const { api } = await import('../api');
    renderPage('/product-conversations/pc-1', true);
    await waitForPageReady();
    const firstOpenId = vi.mocked(api.getProductConversationHistorySnapshot).mock.calls[0]?.[1]?.open_id;
    await waitFor(() => expect(api.reportProductConversationOpen).toHaveBeenCalledTimes(1));
    expect(api.reportProductConversationOpen).toHaveBeenLastCalledWith(
      expect.objectContaining({ open_id: firstOpenId }),
    );
    fireEvent.click(screen.getByRole('button', { name: 'open second product' }));
    await waitFor(() => expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(2));
    const secondOpenId = vi.mocked(api.getProductConversationHistorySnapshot).mock.calls[1]?.[1]?.open_id;
    fireEvent.click(screen.getByRole('button', { name: 'open first product' }));
    await waitFor(() => expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(3));
    const revisitOpenId = vi.mocked(api.getProductConversationHistorySnapshot).mock.calls[2]?.[1]?.open_id;
    await waitFor(() => expect(api.reportProductConversationOpen).toHaveBeenCalledTimes(2));
    expect(api.reportProductConversationOpen).toHaveBeenLastCalledWith(
      expect.objectContaining({ open_id: revisitOpenId }),
    );
    expect(firstOpenId).toMatch(/^[0-9a-f-]{36}$/);
    expect(secondOpenId).toMatch(/^[0-9a-f-]{36}$/);
    expect(revisitOpenId).toMatch(/^[0-9a-f-]{36}$/);
    expect(new Set([firstOpenId, secondOpenId, revisitOpenId]).size).toBe(3);
  });

  it('reuses one open id across StrictMode effect replay', async () => {
    const { api } = await import('../api');
    renderPage('/product-conversations/pc-strict', false, true);
    await waitForPageReady();
    const correlatedCalls = vi.mocked(api.getProductConversationHistorySnapshot).mock.calls
      .map(([, options]) => options?.open_id)
      .filter(Boolean);
    expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(1);
    expect(new Set(correlatedCalls).size).toBe(1);
    await waitFor(() => expect(api.reportProductConversationOpen).toHaveBeenCalledTimes(1));
  });

  it('reuses the open correlation when an initial snapshot retry succeeds', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot)
      .mockRejectedValueOnce(new Error('temporary failure'))
      .mockResolvedValueOnce(makeSnapshot());
    renderPage('/product-conversations/pc-retry');

    expect(await screen.findByRole('alert')).toHaveTextContent('temporary failure');
    const firstOpenId = vi.mocked(api.getProductConversationHistorySnapshot).mock.calls[0]?.[1]?.open_id;
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    await waitForPageReady();
    expect(vi.mocked(api.getProductConversationHistorySnapshot).mock.calls[1]?.[1]?.open_id).toBe(firstOpenId);
    await waitFor(() => expect(api.reportProductConversationOpen).toHaveBeenCalledTimes(1));
  });

  it('does not emit an uncorrelated ordinary-open journey for Global history', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getCoordinatorHistory).mockResolvedValue(coordinatorSnapshot(makeSnapshot()));
    renderPage('/global/row-2');
    await waitForPageReady();
    expect(api.getCoordinatorHistory).toHaveBeenCalledWith('row-1', {
      message_limit: 100, open_id: expect.any(String), onTelemetry: expect.any(Function),
    });
    expect(api.reportProductConversationOpen).not.toHaveBeenCalled();
  });

  it('correlates the initial snapshot with one bounded browser readiness report', async () => {
    const { api } = await import('../api');
    renderPage('/product-conversations/pc-telemetry');
    await waitForPageReady();

    const initialOptions = vi.mocked(api.getProductConversationHistorySnapshot).mock.calls[0]?.[1];
    expect(initialOptions?.open_id).toMatch(/^[0-9a-f-]{36}$/);
    await waitFor(() => expect(api.reportProductConversationOpen).toHaveBeenCalledTimes(1));
    expect(api.reportProductConversationOpen).toHaveBeenCalledWith(expect.objectContaining({
      open_id: initialOptions?.open_id,
      visible: true,
    }));
    const report = vi.mocked(api.reportProductConversationOpen).mock.calls[0]?.[0];
    expect(report).toBeDefined();
    if (!report) throw new Error('expected product conversation open report');
    expect(report.snapshot_received_ms).toBeLessThanOrEqual(report.store_ready_ms);
    expect(report.first_paint_ms).not.toBeNull();
    if (report.first_paint_ms === null) throw new Error('expected visible first paint');
    expect(report.store_ready_ms).toBeLessThanOrEqual(report.first_paint_ms);
    expect(report.first_paint_ms).toBeLessThanOrEqual(report.total_ms);
  });

  it('reconciles an aligned SSE prefix into monotonic latest-row chronology', async () => {
    const { api } = await import('../api');
    const scenario = getProductConversationScenario('latest-row-aligned-prefix-tail');
    if (!scenario.snapshot || !scenario.alignedLatestMessages) throw new Error('fixture scenario incomplete');
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValue(scenario.snapshot);
    renderPage('/product-conversations/pc-aligned-prefix-tail');
    await waitForPageReady();
    act(() => emitLatestProjection({
      conversationId: 'row-aligned-prefix-tail',
      slug: 'aligned-prefix-tail',
      messages: scenario.alignedLatestMessages,
      pendingMessages: [],
      convState: { type: 'idle' },
    }));

    const props = conversationNavStackSpy.mock.calls.at(-1)?.[0];
    const messages = props.messages as Message[];
    const sequenceKeys = messages.map((message) => [message.sequence_id, message.message_id] as const);
    expect(sequenceKeys).toEqual([...sequenceKeys].sort(([leftSequence, leftId], [rightSequence, rightId]) => (
      leftSequence - rightSequence || leftId.localeCompare(rightId)
    )));
    expect(messages).toHaveLength(101);
    expect(messages.at(-1)?.message_id).toBe('aligned-prefix-terminal-assistant');
    expect(JSON.stringify(messages.at(-1)?.content)).toContain(ALIGNED_PREFIX_TERMINAL_MARKER);

    const { historicalUnits } = buildHistoricalUnits({ messages, pendingMessages: [] });
    expect(historicalUnits.at(-1)?.key).toBe('aligned-prefix-terminal-assistant');
    const completedToolIds = historicalUnits.flatMap((unit) => {
      if (unit.kind === 'agent_turn') return [...unit.toolResultsByUseId.keys()];
      if (unit.kind === 'tool_only_agent_turn_group') {
        return unit.members.flatMap((member) => [...member.toolResultsByUseId.keys()]);
      }
      return [];
    });
    expect(completedToolIds).toContain(ALIGNED_PREFIX_BOUNDARY_TOOL_ID);
    expect(completedToolIds).toContain(ALIGNED_PREFIX_STEERING_TOOL_ID);
  });

  it('lets a same-sequence live row replace the stale snapshot identity', async () => {
    const { api } = await import('../api');
    const scenario = getProductConversationScenario('latest-row-aligned-prefix-tail');
    if (!scenario.snapshot || !scenario.alignedLatestMessages) throw new Error('fixture scenario incomplete');
    const snapshot = structuredClone(scenario.snapshot);
    const latest = snapshot.segments[0];
    if (!latest) throw new Error('fixture snapshot missing latest segment');
    latest.messages = latest.messages.map((message) => message.sequence_id === 101
      ? { ...message, message_id: 'stale-terminal-identity' }
      : message);
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValue(snapshot);

    renderPage('/product-conversations/pc-aligned-prefix-tail');
    await waitForPageReady();
    act(() => emitLatestProjection({
      conversationId: 'row-aligned-prefix-tail',
      slug: 'aligned-prefix-tail',
      messages: scenario.alignedLatestMessages,
      pendingMessages: [],
      convState: { type: 'idle' },
    }));

    const messages = conversationNavStackSpy.mock.calls.at(-1)?.[0].messages as Message[];
    expect(messages.filter((message) => message.sequence_id === 101)).toEqual([
      expect.objectContaining({ message_id: 'aligned-prefix-terminal-assistant' }),
    ]);
    expect(messages.some((message) => message.message_id === 'stale-terminal-identity')).toBe(false);
  });

  it('ignores a stale live projection from the prior latest transcript row', async () => {
    const { api } = await import('../api');
    const scenario = getProductConversationScenario('latest-row-aligned-prefix-tail');
    if (!scenario.snapshot) throw new Error('fixture scenario incomplete');
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValue(scenario.snapshot);

    renderPage('/product-conversations/pc-aligned-prefix-tail');
    await waitForPageReady();
    act(() => emitLatestProjection({
      conversationId: 'prior-latest-row',
      slug: 'prior-latest-row',
      messages: [{
        message_id: 'foreign-row-message',
        conversation_id: 'prior-latest-row',
        sequence_id: 999,
        message_type: 'agent',
        content: [{ type: 'text', text: 'must not cross the row boundary' }],
        display_data: {},
        usage_data: null,
        created_at: '2026-07-01T12:00:00Z',
      }],
      pendingMessages: [{
        localId: 'foreign-pending',
        text: 'must not appear after the current terminal tail',
        queuedAt: Date.now(),
        queuePosition: 1,
      }],
      convState: {
        type: 'tool_executing',
        toolName: 'foreign-tool',
        toolInput: '{}',
        remainingTools: 0,
      },
    }));

    const props = conversationNavStackSpy.mock.calls.at(-1)?.[0];
    const messages = props.messages as Message[];
    expect(messages.some((message) => message.message_id === 'foreign-row-message')).toBe(false);
    expect(messages.at(-1)?.message_id).toBe('aligned-prefix-terminal-assistant');
    expect(props.pendingMessages).toEqual([]);
    expect(props.convState).toEqual({ type: 'idle' });
    expect(props.conversationId).toBe('pc-aligned-prefix-tail');
  });

  it('refetches the authoritative aggregate when SSE changes member archive state', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot)
      .mockResolvedValueOnce(makeSnapshot({ ordinary_lifecycle: 'open' }))
      .mockResolvedValueOnce(makeSnapshot({ ordinary_lifecycle: 'history' }));

    renderPage();
    await waitForPageReady();

    act(() => emitLatestProjection({ isArchived: false, serverArchived: false }));
    expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(1);
    act(() => emitLatestProjection({ isArchived: false, serverArchived: true }));

    await waitFor(() => expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.getByText('History is read-only.')).toBeInTheDocument());
    expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(2);
  });

  it('orders flattened segment messages chronologically', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(makeSnapshot({
      segments: [
        { segment_ordinal: 2, transcript_row_id: 'row-2', slug: null, title: null, messages: [makeMessage('m-3', 3, 'row-2'), makeMessage('m-4', 4, 'row-2')], handoff: null },
        { segment_ordinal: 1, transcript_row_id: 'row-1', slug: null, title: null, messages: [makeMessage('m-1', 1, 'row-1'), makeMessage('m-2', 2, 'row-1')], handoff: null },
      ],
    }));

    renderPage();

    await waitFor(() => {
      expect(screen.getByTestId('message-order').textContent).toBe('m-1,m-2,m-3,m-4');
    });
  });

  it('renders every typed historical handoff exactly once at the segment boundary', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(makeSnapshot({
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
        'm-1,m-2,cont-1,m-3,m-4,cont-2,m-5,m-6,cont-3'
      );
    });
    expect(screen.getByTestId('message-types').textContent).toBe('user,agent,continuation,user,agent,continuation,user,agent,continuation');
    expect(screen.getByTestId('message-text-order').textContent).toContain('First handoff');
    expect(screen.getByTestId('message-text-order').textContent).toContain('Second handoff');
    expect(screen.getByTestId('message-text-order').textContent).toContain('Third handoff');
    const messages = conversationNavStackSpy.mock.lastCall?.[0]?.['messages'] as Message[];
    expect(messages.filter((message) => message.message_type === 'continuation').map((message) => message.message_id)).toEqual(['cont-1', 'cont-2', 'cont-3']);
    expect(messages.every((message) => message.sequence_id > 0)).toBe(true);
  });

  it('renders completed handoffs as the single typed segment boundary', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(makeSnapshot({
      segments: [
        {
          segment_ordinal: 1,
          transcript_row_id: 'row-1',
          slug: 'row-1',
          title: 'Earlier',
          messages: [makeMessage('m-1', 1, 'row-1'), makeMessage('m-2', 2, 'row-1')],
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
            opening_authority: 'user_authorized_instruction',
            summary: 'Completed handoff',
          },
        },
      ],
    }));

    renderPage();

    await waitFor(() => {
      expect(screen.getByTestId('message-order').textContent).toBe(
        'm-1,m-2,accepted-successor,continuation-request'
      );
    });
    expect(screen.getByTestId('message-text-order')).toHaveTextContent('Completed handoff');
  });

  it('keeps exact order across 100+ messages spanning multiple paginated segments', async () => {
    const { api } = await import('../api');
    const segment0Messages = Array.from({ length: 35 }, (_, index) => makeMessage(`m-${index + 1}`, index + 1, 'conv-0'));
    const segment1Messages = Array.from({ length: 35 }, (_, index) => makeMessage(`m-${index + 36}`, index + 36, 'conv-1'));
    const segment2Messages = Array.from({ length: 40 }, (_, index) => makeMessage(`m-${index + 71}`, index + 71, 'conv-2'));
    vi.mocked(api.getProductConversationHistorySnapshot)
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
    expect(order[35]).toBe('cont-1');
    expect(order[71]).toBe('cont-2');
    expect(order[111]).toBe('m-110');
    expect(order.at(-1)).toBe('cont-3');
    expect(new Set(order).size).toBe(order.length);
  });

  it('assigns occurrence identity to newly streamed latest messages', async () => {
    renderPage();
    await waitForPageReady();
    emitLatestProjection({ messages: [makeMessage('new-live', 7, 'row-2')] });

    await waitFor(() => {
      const props = conversationNavStackSpy.mock.lastCall?.[0] as { messages: Array<{ message_id: string; display_data?: { productOccurrenceToken?: string } }> };
      expect(props.messages.find((message) => message.message_id === 'new-live')?.display_data?.productOccurrenceToken)
        .toBe('row-2:new-live');
    });
  });

  it('shows loading then request error state', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot).mockRejectedValueOnce(new Error('boom'));

    renderPage();

    expect(screen.getByTestId('skeleton')).toBeInTheDocument();
    expect(await screen.findByRole('alert')).toHaveTextContent('boom');
  });

  it('surfaces pagination failure through aggregated status', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot)
      .mockResolvedValueOnce(makeSnapshot())
      .mockRejectedValueOnce(new Error('older failed'));

    renderPage();
    await waitForPageReady();

    fireEvent.click(screen.getByRole('button', { name: 'load older' }));

    await waitFor(() => {
      expect(screen.getByText('older failed')).toBeInTheDocument();
    });
  });

  it('uses one ordinary transcript and embeds the writable ordinary composer without diagnostics', async () => {
    renderPage();
    await waitForPageReady();

    const page = screen.getByTestId('product-conversation-page');
    expect(page.querySelectorAll('#chat-view')).toHaveLength(0);
    expect(screen.getByTestId('embedded-conversation-page')).toHaveTextContent('embedded row-2');
    expect(embeddedConversationPageSpy).toHaveBeenLastCalledWith(expect.objectContaining({
      slug: 'row-2',
      suppressCanonicalization: true,
      mutationEnabled: true,
      showTranscript: false,
    }));
    expect(page).not.toHaveTextContent('Presentation');
    expect(page).not.toHaveTextContent('Lifecycle');
    expect(page).not.toHaveTextContent('Q&A history');
    expect(page).not.toHaveTextContent('Aggregate');
    expect(page.querySelector('.product-conversation-page__layout')).toBeNull();
  });


  it('renders the aggregate title and present source link for Open and History', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot)
      .mockResolvedValueOnce(makeSnapshot({ presentation: { kind: 'state', display_name: 'Open title', presentation_mode: 'idle' } }))
      .mockResolvedValueOnce(makeSnapshot({
        ordinary_lifecycle: 'history',
        writable_transcript_row_id: null,
        presentation: { kind: 'state', display_name: 'History title', presentation_mode: 'done' },
      }));

    const { unmount } = renderPage();
    await waitForPageReady();
    expect(screen.getByRole('heading', { name: 'Open title' })).toBeInTheDocument();
    const source = screen.getByTestId('product-conversation-source');
    expect(source).toHaveTextContent('Approved task from source conversation');
    expect(screen.getByRole('link', { name: 'source conversation' })).toHaveAttribute('href', '/c/pc-source');

    unmount();
    renderPage();
    await waitForPageReady();
    expect(screen.getByRole('heading', { name: 'History title' })).toBeInTheDocument();
    expect(screen.getByTestId('product-conversation-history')).toHaveTextContent('History is read-only.');
    expect(screen.getByRole('link', { name: 'source conversation' })).toBeInTheDocument();
  });

  it('renders deleted source copy without a dead source link', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(makeSnapshot({
      source: { ...makeSnapshot().source!, status: 'deleted' },
    }));
    renderPage();
    await waitForPageReady();

    expect(screen.getByTestId('product-conversation-source')).toHaveTextContent('Approved task source unavailable or deleted');
    expect(screen.queryByRole('link', { name: 'source conversation' })).not.toBeInTheDocument();
  });

  it('keeps Work collapsed until opened, then shows typed work and PR health in Open and History', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getPrStatus).mockResolvedValue({
      found: true,
      number: 248,
      display_state: 'open',
      check_state: 'passing',
      feedback_freshness: { state: 'new', count: 3 },
      work_change: { kind: 'clean' },
      refresh: { state: 'fresh', stale: false, last_attempted_at: '2026-01-01T00:00:00Z' },
    });
    vi.mocked(api.getProductConversationHistorySnapshot)
      .mockResolvedValueOnce(makeSnapshot())
      .mockResolvedValueOnce(makeSnapshot({ ordinary_lifecycle: 'history', writable_transcript_row_id: null }));

    const { unmount } = renderPage();
    await waitForPageReady();
    const work = screen.getByTestId('product-conversation-work');
    expect(work).not.toHaveAttribute('open');
    fireEvent.click(screen.getByText('Work'));
    expect(work).toHaveAttribute('open');
    expect(screen.getByTitle('feature/test → main')).toBeInTheDocument();
    expect(screen.getByText('/tmp/worktree')).toBeInTheDocument();
    expect(screen.getByText('40012')).toBeInTheDocument();
    expect(screen.getByTitle('Product foundation')).toBeInTheDocument();
    expect(await screen.findByText('#248 checks ✓')).toBeInTheDocument();
    expect(screen.getByText('3 new')).toBeInTheDocument();

    unmount();
    renderPage();
    await waitForPageReady();
    expect(screen.getByTestId('product-conversation-history')).toBeInTheDocument();
    expect(screen.getByTestId('product-conversation-work')).toBeInTheDocument();
  });

  it('uses a split-pane viewer only at wide desktop and keeps the narrow viewer overlay-owned', async () => {
    const wide = renderPage('/product-conversations/pc-1?viewer=message&presentation=pane&message=3&message_id=m-3');
    await waitForPageReady();
    expect(screen.getByTestId('product-conversation-page')).toHaveClass('product-conversation-page--split-pane');
    expect(await screen.findByTestId('aggregate-message-viewer')).toHaveAttribute('data-inline', 'true');
    expect(screen.getByTestId('aggregate-message-viewer').parentElement).toHaveClass('product-conversation-page__viewer-pane');
    wide.unmount();

    viewportFlags.isWideDesktop = false;
    renderPage('/product-conversations/pc-1?viewer=message&presentation=pane&message=3&message_id=m-3');
    await waitForPageReady();
    expect(screen.getByTestId('product-conversation-page')).not.toHaveClass('product-conversation-page--split-pane');
    expect(await screen.findByTestId('aggregate-message-viewer')).toHaveAttribute('data-inline', 'false');
    expect(screen.getByTestId('aggregate-message-viewer').parentElement).not.toHaveClass('product-conversation-page__viewer-pane');
  });

  it('routes aggregate viewer notes to the embedded latest-row composer capability', async () => {
    const appendReviewNotesToComposer = vi.fn();
    renderPage('/product-conversations/pc-1?viewer=message&presentation=pane&message=3&message_id=m-3');
    await waitForPageReady();

    act(() => emitLatestProjection({ appendReviewNotesToComposer }));
    const viewerProps = viewerSpy.mock.lastCall?.[0] as Record<string, unknown> | undefined;
    const onSendNotes = viewerProps?.['onSendNotes'] as ((notes: string) => void) | undefined;
    expect(onSendNotes).toBeTypeOf('function');

    act(() => onSendNotes?.('## Review notes\n\nunique aggregate note'));
    expect(appendReviewNotesToComposer).toHaveBeenCalledWith('## Review notes\n\nunique aggregate note');
    await waitFor(() => expect(screen.queryByTestId('aggregate-message-viewer')).not.toBeInTheDocument());
  });

  it('keeps fullscreen aggregate review open so successful notes can return to its pane', async () => {
    const appendReviewNotesToComposer = vi.fn();
    renderPage('/product-conversations/pc-1?viewer=message&presentation=fullscreen&message=3&message_id=m-3');
    await waitForPageReady();

    act(() => emitLatestProjection({ appendReviewNotesToComposer }));
    const viewerProps = viewerSpy.mock.lastCall?.[0] as Record<string, unknown> | undefined;
    const onSendNotes = viewerProps?.['onSendNotes'] as ((notes: string) => void) | undefined;
    act(() => onSendNotes?.('## Review notes\n\nfocused aggregate note'));

    expect(appendReviewNotesToComposer).toHaveBeenCalledWith('## Review notes\n\nfocused aggregate note');
    expect(screen.getByTestId('aggregate-message-viewer')).toBeInTheDocument();
  });

  it('does not expose note sending while the latest row shows a question-only response panel', async () => {
    renderPage('/product-conversations/pc-1?viewer=message&presentation=pane&message=3&message_id=m-3');
    await waitForPageReady();
    act(() => emitLatestProjection({
      convState: {
        type: 'awaiting_user_response',
        questions: [{
          question: 'Choose a direction',
          header: 'Direction',
          options: [{ label: 'A', description: 'First' }, { label: 'B', description: 'Second' }],
          multiSelect: false,
        }],
        answers: [],
      },
      appendReviewNotesToComposer: undefined,
    }));

    const viewerProps = viewerSpy.mock.lastCall?.[0] as Record<string, unknown> | undefined;
    expect(viewerProps?.['onSendNotes']).toBeUndefined();
  });

  it('does not expose note sending when History has no composer capability', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(makeSnapshot({
      ordinary_lifecycle: 'history',
      writable_transcript_row_id: null,
    }));
    renderPage('/product-conversations/pc-1?viewer=message&presentation=pane&message=3&message_id=m-3');
    await waitForPageReady();

    const viewerProps = viewerSpy.mock.lastCall?.[0] as Record<string, unknown> | undefined;
    expect(viewerProps?.['onSendNotes']).toBeUndefined();
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
    vi.mocked(api.getProductConversationHistorySnapshot).mockRejectedValueOnce(new Error('snapshot failed'));

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
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(makeSnapshot({
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
        'm-1,m-2,cont-1,m-3,m-4,live-sent,live-streamed'
      );
      expect(conversationNavStackSpy.mock.lastCall?.[0]?.['pendingMessages']).toEqual([
        expect.objectContaining({ localId: 'pending-local', status: 'pending' }),
      ]);
    });
  });

  it('lets projected latest state drive aggregate nav metadata', async () => {
    renderPage();
    await waitForPageReady();

    emitLatestProjection({ slug: 'row-live-2', conversationId: 'row-2', conversation: { work_scope_key: 'ws-latest' } });

    await waitFor(() => {
      const props = conversationNavStackSpy.mock.lastCall?.[0] as Record<string, unknown>;
      expect(props['slug']).toBe('row-2');
      expect(props['conversationId']).toBe('pc-1');
      expect(props['workScopeKey']).toBe('ws-latest');
    });
  });

  it.each([
    ['archived_cleanup_attention', 'history', 'Closed — cleanup needs attention', 'tmux_server'],
    ['close_incomplete', 'open', 'Close incomplete — shutdown could not be confirmed', 'bash_process_group'],
    ['close_incomplete', 'open', 'Close incomplete — shutdown could not be confirmed', 'work_scope'],
  ] as const)('retains %s status and read-only controls on cold load and remount (%s, %s, %s)', async (outcome, lifecycle, label, resourceKind) => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValue(makeSnapshot({
      ordinary_lifecycle: lifecycle,
      writable_transcript_row_id: null,
      close: {
        attempt_id: 'stopped-close', phase: 'completed', outcome,
        run_ordinal: '2', run_status: 'stopped',
        confirmation_snapshot: null, inspections: [], losses: [],
        residuals: [],
        failure: {
          occurrence_id: 'close-failure:2:stopped-close',
          reason: 'identity_not_proven', detail: 'Exact failure before inventory',
          stop_certainty: lifecycle === 'history'
            ? { kind: 'conversation_and_processes_stopped', confirmed_at_us: '1234567890123456' }
            : { kind: 'shutdown_uncertain' },
          remaining_resources: [
            { scope: 'scope-1', resource_kind: resourceKind, identity: 'epoch:failed', disposition: 'failed' },
            { scope: 'scope-2', resource_kind: 'worktree', identity: 'worktree:retained', disposition: 'unattempted' },
            { scope: 'scope-1', resource_kind: 'pty_session', identity: 'epoch:unproven', disposition: 'unknown' },
            { scope: 'scope-3', resource_kind: 'browser_session', identity: 'browser:residual', disposition: 'residual' },
          ],
        },
      },
    }));

    const firstMount = renderPage();
    for (const reload of [false, true]) {
      if (reload) {
        firstMount.unmount();
        localStorage.clear();
        renderPage();
      }
      const status = await screen.findByRole('alert', { name: 'Close status' });
      expect(status).toHaveTextContent(label);
      expect(status).toHaveTextContent('Run 2 · stopped. No automatic retry.');
      expect(status).toHaveTextContent('identity not proven — Exact failure before inventory');
      expect(status).toHaveTextContent('Failure close-failure:2:stopped-close');
      expect(status).toHaveTextContent(lifecycle === 'history' ? 'Conversation and processes stopped.' : 'Shutdown uncertain.');
      fireEvent.click(screen.getByText('Remaining resources (4)'));
      const resources = status.querySelectorAll('ol li');
      expect(resources).toHaveLength(4);
      for (const [index, text] of [
        `scope-1: ${resourceKind.replaceAll('_', ' ')} · epoch:failed · failed`,
        'scope-2: worktree · worktree:retained · unattempted',
        'scope-1: pty session · epoch:unproven · unknown',
        'scope-3: browser session · browser:residual · residual',
      ].entries()) {
        expect(resources[index]).toBeVisible();
        expect(resources[index]).toHaveTextContent(text);
      }
      expect(screen.queryByTestId('product-conversation-composer')).not.toBeInTheDocument();
      expect(screen.getByTestId('product-conversation-close-live-update-owner')).not.toBeVisible();
      expect(embeddedConversationPageSpy).not.toHaveBeenCalled();
      expect(screen.queryByRole('button', { name: /Retry exact Close/ })).not.toBeInTheDocument();
      fireEvent.click(screen.getByRole('button', { name: 'Recall' }));
      await waitFor(() => expect(chainQaColumnSpy.mock.lastCall?.[0]?.['disabled']).toBe(true));
      expect(conversationNavStackSpy.mock.lastCall?.[0]?.['convState']).toEqual({ type: 'idle' });
      expect(screen.getByTestId(lifecycle === 'history'
        ? 'product-conversation-history' : 'product-conversation-close-incomplete')).toBeVisible();
    }
    expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(2);
  });

  it('refreshes cleanup-attention History to archived through the mounted conversation stream', async () => {
    const { api } = await import('../api');
    const failure = {
      occurrence_id: 'close-failure:2:stopped-close',
      reason: 'identity_not_proven' as const,
      detail: 'Exact cleanup failure',
      stop_certainty: {
        kind: 'conversation_and_processes_stopped' as const,
        confirmed_at_us: '1234567890123456',
      },
      remaining_resources: [
        {
          scope: 'scope-1',
          resource_kind: 'worktree' as const,
          identity: 'worktree:retained',
          disposition: 'failed' as const,
        },
      ],
    };
    const cleanupAttention = makeSnapshot({
      ordinary_lifecycle: 'history',
      writable_transcript_row_id: null,
      close: {
        attempt_id: 'stopped-close',
        phase: 'completed',
        outcome: 'archived_cleanup_attention',
        run_ordinal: '2',
        run_status: 'stopped',
        confirmation_snapshot: null,
        inspections: [],
        losses: [],
        residuals: [],
        failure,
      },
    });
    const archived = makeSnapshot({
      ordinary_lifecycle: 'history',
      writable_transcript_row_id: null,
      close: {
        attempt_id: 'stopped-close',
        phase: 'completed',
        outcome: 'archived',
        run_ordinal: '3',
        run_status: 'completed',
        confirmation_snapshot: null,
        inspections: [],
        losses: [],
        residuals: [],
        failure: null,
      },
    });
    vi.mocked(api.getProductConversationHistorySnapshot)
      .mockResolvedValueOnce(cleanupAttention)
      .mockResolvedValueOnce(archived);

    renderPage();
    expect(await screen.findByRole('alert', { name: 'Close status' }))
      .toHaveTextContent('Closed — cleanup needs attention');
    expect(screen.getByTestId('product-conversation-close-live-update-owner')).not.toBeVisible();
    expect(embeddedConversationPageSpy).not.toHaveBeenCalled();
    const stream = FakeCloseEventSource.instances.at(-1);
    expect(stream?.url).toContain('/api/conversations/row-2/stream');

    act(() => stream?.emit('conversation_update', {
      sequence_id: 1,
      conversation: { archived: true },
    }));

    await waitFor(() => expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(2));
    await waitFor(() => {
      expect(screen.queryByRole('alert', { name: 'Close status' })).not.toBeInTheDocument();
      expect(screen.queryByTestId('product-conversation-close-live-update-owner')).not.toBeInTheDocument();
      expect(screen.getByTestId('product-conversation-history')).toBeVisible();
    });
  });

  it('disables the ordinary composer while Close is active', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(makeSnapshot({
      close: {
        attempt_id: 'close-active',
        phase: 'settling_active_work', outcome: null, run_ordinal: '1', run_status: 'running', failure: null,
        confirmation_snapshot: null,
        inspections: [],
        losses: [],
        residuals: [],
      },
    }));

    renderPage();
    await waitForPageReady();

    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.['mutationEnabled']).toBe(false);
  });

  it('does not expose aggregate note sending while Close makes the composer read-only', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(makeSnapshot({
      close: {
        attempt_id: 'close-active',
        phase: 'settling_active_work', outcome: null, run_ordinal: '1', run_status: 'running', failure: null,
        confirmation_snapshot: null,
        inspections: [],
        losses: [],
        residuals: [],
      },
    }));

    renderPage('/product-conversations/pc-1?viewer=message&presentation=pane&message=3&message_id=m-3');
    await waitForPageReady();
    act(() => emitLatestProjection({ appendReviewNotesToComposer: undefined }));

    const viewerProps = viewerSpy.mock.lastCall?.[0] as Record<string, unknown> | undefined;
    expect(viewerProps?.['onSendNotes']).toBeUndefined();
  });

  it('background-refreshes active Close phases without replacing the page with a skeleton', async () => {
    const { api } = await import('../api');
    const activeClose = {
      attempt_id: 'close-active',
      phase: 'settling_active_work', outcome: null, run_ordinal: '1', run_status: 'running', failure: null,
      confirmation_snapshot: null,
      inspections: [],
      losses: [],
      residuals: [],
    } satisfies ProductConversationCloseView;
    let resolveRefresh!: (value: ProductConversationSnapshotView) => void;
    const refresh = new Promise<ProductConversationSnapshotView>((resolve) => { resolveRefresh = resolve; });
    vi.mocked(api.getProductConversationHistorySnapshot)
      .mockResolvedValueOnce(makeSnapshot({ close: activeClose }))
      .mockReturnValueOnce(refresh);

    renderPage();
    await waitForPageReady();
    act(() => notifyCloseSnapshotChanged('row-2', 'stream'));

    await waitFor(() => expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(2));
    expect(screen.queryByTestId('skeleton')).not.toBeInTheDocument();
    expect(screen.getByTestId('message-order')).toHaveTextContent('m-1,m-2');
    await act(async () => resolveRefresh(makeSnapshot({
      close: { ...activeClose, phase: 'awaiting_retirement_inspection' },
    })));
    expect(screen.getByTestId('message-order')).toHaveTextContent('m-1,m-2');
  });

  it('preserves loaded history across active Close refreshes', async () => {
    const { api } = await import('../api');
    const activeClose = {
      attempt_id: 'close-active',
      phase: 'settling_active_work', outcome: null, run_ordinal: '1', run_status: 'running', failure: null,
      confirmation_snapshot: null,
      inspections: [],
      losses: [],
      residuals: [],
    } satisfies ProductConversationCloseView;
    vi.mocked(api.getProductConversationHistorySnapshot)
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
            messages: [makeMessage('m-3', 3, 'row-2'), makeMessage('m-4', 4, 'row-2')],
            handoff: null,
          },
        ],
      }));

    renderPage();
    await waitForPageReady();
    fireEvent.click(screen.getByRole('button', { name: 'load older' }));
    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent('old-message'));

    act(() => notifyCloseSnapshotChanged('row-2', 'stream'));

    await waitFor(() => expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(3));
    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent('old-message'));
    expect(screen.getByTestId('message-order')).toHaveTextContent('old-message,m-1,m-2,cont-1,m-3,m-4');
    expect(screen.getByTestId('message-order').textContent?.split(',').filter((id) => id === 'old-message')).toHaveLength(1);
    expect(conversationNavStackSpy.mock.lastCall?.[0]?.['transcriptPositioning']).toEqual(expect.objectContaining({ kind: 'idle' }));
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.['mutationEnabled']).toBe(false);
  });

  it('does not retain a removed segment inside the authoritative refreshed tail range', async () => {
    const { api } = await import('../api');
    const activeClose = {
      attempt_id: 'close-active',
      phase: 'settling_active_work', outcome: null, run_ordinal: '1', run_status: 'running', failure: null,
      confirmation_snapshot: null,
      inspections: [],
      losses: [],
      residuals: [],
    } satisfies ProductConversationCloseView;
    vi.mocked(api.getProductConversationHistorySnapshot)
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
            messages: [makeMessage('m-3', 3, 'row-2'), makeMessage('m-4', 4, 'row-2')],
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
          messages: [makeMessage('m-3', 3, 'row-2'), makeMessage('m-4', 4, 'row-2')],
          handoff: null,
        }],
      }));
    renderPage();
    await waitForPageReady();

    act(() => notifyCloseSnapshotChanged('pc-1', 'stream'));

    await waitFor(() => expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.getByTestId('message-order')).not.toHaveTextContent('stale-message'));
    expect(screen.getByTestId('message-order')).toHaveTextContent('m-3,m-4');
  });

  it('does not preserve stale cached segments when the refreshed tail is empty', async () => {
    const { api } = await import('../api');
    const activeClose = {
      attempt_id: 'close-active',
      phase: 'settling_active_work', outcome: null, run_ordinal: '1', run_status: 'running', failure: null,
      confirmation_snapshot: null,
      inspections: [],
      losses: [],
      residuals: [],
    } satisfies ProductConversationCloseView;
    vi.mocked(api.getProductConversationHistorySnapshot)
      .mockResolvedValueOnce(makeSnapshot({ close: activeClose }))
      .mockResolvedValueOnce(makeSnapshot({
        close: { ...activeClose, phase: 'awaiting_retirement_inspection' },
        segments: [],
      }));
    renderPage();
    await waitForPageReady();

    act(() => notifyCloseSnapshotChanged('pc-1', 'stream'));

    await waitFor(() => expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.getByTestId('message-count')).toHaveTextContent('0'));
  });

  it('adopts a refreshed pagination boundary when no loaded prefix was retained', async () => {
    const { api } = await import('../api');
    const activeClose = {
      attempt_id: 'close-active',
      phase: 'settling_active_work', outcome: null, run_ordinal: '1', run_status: 'running', failure: null,
      confirmation_snapshot: null,
      inspections: [],
      losses: [],
      residuals: [],
    } satisfies ProductConversationCloseView;
    vi.mocked(api.getProductConversationHistorySnapshot)
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

    await waitFor(() => expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(conversationNavStackSpy.mock.lastCall?.[0]?.['hasOlderMessages']).toBe(true));
  });

  it('does not refetch an Open snapshot for ordinary state invalidations', async () => {
    const { api } = await import('../api');
    renderPage();
    await waitForPageReady();

    act(() => notifyCloseSnapshotChanged('row-2', 'stream'));
    await act(async () => Promise.resolve());

    expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(1);
    expect(screen.queryByTestId('skeleton')).not.toBeInTheDocument();
  });

  it('discovers an explicitly-started Close while the loaded snapshot is Open', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot)
      .mockResolvedValueOnce(makeSnapshot())
      .mockResolvedValueOnce(makeSnapshot({
        close: {
          attempt_id: 'close-started-elsewhere',
          phase: 'settling_active_work', outcome: null, run_ordinal: '1', run_status: 'running', failure: null,
          confirmation_snapshot: null,
          inspections: [],
          losses: [],
          residuals: [],
        },
      }));
    renderPage();
    await waitForPageReady();

    act(() => notifyCloseSnapshotChanged('row-2'));

    await waitFor(() => expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.['mutationEnabled']).toBe(false));
  });

  it('renders History as the aggregate transcript with no ordinary composer', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(makeSnapshot({
      writable_transcript_row_id: null,
      ordinary_lifecycle: 'history',
    }));

    renderPage();
    await waitForPageReady();

    expect(screen.getByTestId('product-conversation-history')).toHaveTextContent('History is read-only.');
    expect(screen.queryByTestId('embedded-conversation-page')).not.toBeInTheDocument();
    expect(screen.getByTestId('message-order')).toHaveTextContent('cont-1');
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
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(makeSnapshot({
      writable_transcript_row_id: null,
      ordinary_lifecycle: 'open',
    }));

    renderPage();
    await waitForPageReady();

    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.['mutationEnabled']).toBe(true);
  });

  it('keeps an Open aggregate mutable after archived drift triggers a failed background refresh', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot)
      .mockResolvedValueOnce(makeSnapshot({ ordinary_lifecycle: 'open' }))
      .mockRejectedValueOnce(new Error('refresh failed'));
    renderPage();
    await waitForPageReady();

    act(() => emitLatestProjection({ serverArchived: true }));

    expect(await screen.findByRole('alert')).toHaveTextContent('refresh failed');
    expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(2);
    expect(screen.getByTestId('message-order')).toHaveTextContent('m-1,m-2');
    expect(screen.queryByText('Showing cached row if available.')).not.toBeInTheDocument();
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.['mutationEnabled']).toBe(true);
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.['aggregateLifecycleOpen']).toBe(true);
  });

  it('keeps a History aggregate read-only after a failed background refresh despite unarchived drift', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot)
      .mockResolvedValueOnce(makeSnapshot({ ordinary_lifecycle: 'history' }))
      .mockRejectedValueOnce(new Error('refresh failed'));
    renderPage();
    await waitForPageReady();

    act(() => notifyCloseSnapshotChanged('row-2'));

    expect(await screen.findByRole('alert')).toHaveTextContent('refresh failed');
    expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(2);
    expect(screen.getByTestId('message-order')).toHaveTextContent('m-1,m-2');
    expect(screen.getByText('History is read-only.')).toBeInTheDocument();
    expect(screen.queryByTestId('embedded-conversation-page')).not.toBeInTheDocument();
    expect(screen.queryByText('Showing cached row if available.')).not.toBeInTheDocument();
  });
  it('gates task approval mutations behind liveControlsEnabled', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(makeSnapshot({
      close: {
        attempt_id: 'attempt-1',
        phase: 'settling_active_work', outcome: null, run_ordinal: '1', run_status: 'running', failure: null,
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

  it.each([false, true].flatMap(global => [false, true].map(accepted => [global, accepted] as const)))('older pages cannot replace execution authority (Global=%s, current accepted=%s)', async (global, accepted) => {
    const { api } = await import('../api');
    const opening = makeMessage('authority-opening', 1, 'row-2');
    const initial = makeSnapshot({ latest_opening: accepted ? acceptedOpening(opening.message_id, 'separate_message') : pendingOpening(), writable_transcript_row_id: accepted ? 'row-2' : null });
    initial.segments = initial.segments.map(segment => segment.transcript_row_id === 'row-2' ? { ...segment, messages: accepted ? [opening] : [] } : segment);
    const older = makeSnapshot({ latest_opening: accepted ? pendingOpening() : acceptedOpening(opening.message_id, 'boundary'), writable_transcript_row_id: accepted ? null : 'row-2', before: null, has_older: false,
      segments: [{ segment_ordinal: 0, transcript_row_id: 'row-old', slug: 'row-old', title: 'Old', messages: [makeMessage('authority-prefix', 0, 'row-old')], handoff: null }] });
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValueOnce(coordinatorSnapshot(initial)).mockResolvedValueOnce(coordinatorSnapshot(older));
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(initial).mockResolvedValueOnce(older);
    renderPage(global ? '/global/row-1' : '/product-conversations/pc-1');
    await waitForPageReady();
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.mutationEnabled).toBe(accepted);
    fireEvent.click(screen.getByRole('button', { name: 'load older' }));
    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent('authority-prefix'));
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]).toMatchObject({ slug: 'row-2', mutationEnabled: accepted });
    expect(screen.getByTestId('message-order').textContent?.split(',').includes(opening.message_id)).toBe(accepted);
    expect(global ? api.getCoordinatorHistory : api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(2);
  });

  it.each([false, true])('retains newer stream authority while an older page is in flight (Global=%s)', async global => {
    const { api } = await import('../api');
    const initial = makeSnapshot({ latest_opening: pendingOpening(), writable_transcript_row_id: null });
    initial.segments = initial.segments.map(segment => segment.transcript_row_id === 'row-2' ? { ...segment, messages: [] } : segment);
    const older = { ...initial, before: null, has_older: false, segments: [{ ...initial.segments[0]!, messages: [makeMessage('stream-prefix', 0, 'row-1')] }] };
    let release!: () => void;
    const deferred = new Promise<void>(resolve => { release = resolve; });
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValueOnce(coordinatorSnapshot(initial)).mockImplementationOnce(async () => { await deferred; return coordinatorSnapshot(older); });
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(initial).mockImplementationOnce(async () => { await deferred; return older; });
    renderPage(global ? '/global/row-1' : '/product-conversations/pc-1');
    await waitForPageReady();
    fireEvent.click(screen.getByRole('button', { name: 'load older' }));
    act(() => emitLatestProjection({ opening: acceptedOpening('stream-opening', 'separate_message'), messages: [makeMessage('stream-opening', 1, 'row-2')] }));
    await act(async () => { release(); await deferred; });
    expect(screen.getByTestId('message-order')).toHaveTextContent('stream-prefix');
    expect(screen.getByTestId('message-order')).toHaveTextContent('stream-opening');
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.mutationEnabled).toBe(true);
  });

  it.each([false, true])('fresh cursor tail owns provenance from its actual request start (Global=%s)', async global => {
    const { api } = await import('../api');
    const id = 'fresh-cursor-opening';
    const initial = makeSnapshot({ latest_opening: pendingOpening(), writable_transcript_row_id: null });
    const fresh = makeSnapshot({ latest_opening: acceptedOpening(id, 'separate_message'), before: 'fresh-cursor' });
    fresh.segments = fresh.segments.map(segment => segment.transcript_row_id === 'row-2' ? { ...segment, messages: [{ ...makeMessage(id, 1, 'row-2'), content: { text: 'FRESH_TAIL_CONTENT' } }] } : segment);
    const older = { ...initial, before: null, has_older: false, segments: [{ ...initial.segments[0]!, messages: [makeMessage('fresh-prefix', 0, 'row-1')] }] };
    let rejectOlder!: () => void;
    const deferred = new Promise<void>((_resolve, reject) => { rejectOlder = () => reject(new ApiResponseError('stale cursor', 400)); });
    if (global) vi.mocked(api.getCoordinatorHistory).mockResolvedValueOnce(coordinatorSnapshot(initial)).mockImplementationOnce(async () => { await deferred; return coordinatorSnapshot(older); }).mockResolvedValueOnce(coordinatorSnapshot(fresh)).mockResolvedValueOnce(coordinatorSnapshot(older));
    else vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(initial).mockImplementationOnce(async () => { await deferred; return older; }).mockResolvedValueOnce(fresh).mockResolvedValueOnce(older);
    renderPage(global ? '/global/row-1' : '/product-conversations/pc-1');
    await waitForPageReady();
    fireEvent.click(screen.getByRole('button', { name: 'load older' }));
    act(() => emitLatestProjection({ opening: acceptedOpening(id, 'boundary'), messages: [makeMessage(id, 1, 'row-2')] }));
    await act(async () => { rejectOlder(); });
    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent('fresh-prefix'));
    expect(screen.getByTestId('message-text-order')).toHaveTextContent('FRESH_TAIL_CONTENT');
    expect(embeddedConversationPageSpy.mock.lastCall?.[0]?.mutationEnabled).toBe(true);
    expect(global ? api.getCoordinatorHistory : api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(4);
  });

  it.each([false, true])('correlates every older/recovery GET with its original unfinished journey (Global=%s)', async global => {
    const {api} = await import('../api');
    const reporter = new ViewJourneyReporter();
    let result: import('../hooks/conversationViewJourney').JourneyReport | null = null;
    const report = vi.spyOn(ViewJourneyReporter.prototype, 'report').mockImplementation((attempt, outcome) => { result = attempt.measurement.finish(outcome); });
    const journey = new ViewJourneyLifecycle(defaultViewIdentity(global ? 'global' : 'ordinary', 'navigation'), reporter);
    const originalId = journey.openId;
    const initial = makeSnapshot();
    const older = makeSnapshot({segments: [{...initial.segments[0]!, messages: [makeMessage('owned-prefix', 0, 'row-1')]}], before: null, has_older: false});
    const fetchSnapshot = vi.mocked(global ? api.getCoordinatorHistory : api.getProductConversationHistorySnapshot);
    const shape = (value: ProductConversationSnapshotView) => global ? coordinatorSnapshot(value) : value;
    fetchSnapshot.mockResolvedValueOnce(shape(initial)).mockRejectedValueOnce(new ApiResponseError('stale cursor', 400))
      .mockResolvedValueOnce(shape(makeSnapshot({before: 'fresh-owned'}))).mockResolvedValueOnce(shape(older));
    renderPage(global ? '/global/row-1' : '/product-conversations/pc-1', false, false, journey);
    await waitForPageReady();
    expect(report).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', {name: 'load older'}));
    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent('owned-prefix'));
    const attempts = fetchSnapshot.mock.calls.slice(1, 4).map(call => call[1]);
    expect(attempts).toHaveLength(3);
    attempts.forEach((options, index) => {
      expect(options).toMatchObject({[index === 1 ? 'request_id' : 'open_id']: expect.any(String), onTelemetry: expect.any(Function)});
      expect(options).not.toHaveProperty(index === 1 ? 'open_id' : 'request_id');
    });
    const ids = attempts.map(options => (options!.open_id ?? options!.request_id)!);
    expect(new Set(ids).size).toBe(3);
    expect(ids).not.toContain(originalId);
    attempts.forEach((options, index) => options!.onTelemetry!({size_bucket: '0-10k', trace_id: String(index + 1).repeat(32)}));
    journey.finish('incomplete');
    expect(report).toHaveBeenCalledTimes(1);
    const attempt = report.mock.calls[0]![0];
    expect(attempt.openId).toBe(originalId);
    expect((result as import('../hooks/conversationViewJourney').JourneyReport | null)?.correlations.filter(c => c.kind === 'request').map(c => c.id)).toEqual(expect.arrayContaining(ids));
    report.mockRestore();
  });

  it.each([false, true])('does not rebind pending older recovery to a successor journey (Global=%s)', async global => {
    const {api} = await import('../api');
    const journey = new ViewJourneyLifecycle(defaultViewIdentity(global ? 'global' : 'ordinary', 'navigation'), new ViewJourneyReporter());
    let rejectOlder!: (error: Error) => void;
    const pending = new Promise<never>((_resolve, reject) => {rejectOlder = reject;});
    const initial = makeSnapshot();
    const shape = (value: ProductConversationSnapshotView) => global ? coordinatorSnapshot(value) : value;
    const fetchSnapshot = vi.mocked(global ? api.getCoordinatorHistory : api.getProductConversationHistorySnapshot);
    fetchSnapshot.mockResolvedValueOnce(shape(initial)).mockReturnValueOnce(pending)
      .mockResolvedValueOnce(shape(makeSnapshot({before: 'resumed-cursor'})))
      .mockResolvedValueOnce(shape(makeSnapshot({has_older: false, before: null})));
    const register = vi.spyOn(journey, 'requestStarted');
    renderPage(global ? '/global/row-1' : '/product-conversations/pc-1', false, false, journey);
    await waitForPageReady();
    fireEvent.click(screen.getByRole('button', {name: 'load older'}));
    expect(fetchSnapshot.mock.calls[1]![1]).toMatchObject({open_id: expect.any(String), onTelemetry: expect.any(Function)});
    const registeredBeforeResume = register.mock.calls.length;
    journey.resume();
    await act(async () => {rejectOlder(new ApiResponseError('stale cursor', 400));});
    await waitFor(() => expect(fetchSnapshot).toHaveBeenCalledTimes(4));
    for (const [, options] of fetchSnapshot.mock.calls.slice(2, 4)) {
      expect(options).not.toHaveProperty('open_id');
      expect(options).not.toHaveProperty('onTelemetry');
      expect(options).not.toHaveProperty('request_id');
    }
    expect(register).toHaveBeenCalledTimes(registeredBeforeResume);
    journey.finish('incomplete');
    register.mockRestore();
  });

  it('retries older-page fetch once with a fresh tail cursor after a stale 400 cursor error', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot)
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

    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent('old-message,m-1,m-2,cont-1,m-3,m-4'));
    expect(vi.mocked(api.getProductConversationHistorySnapshot).mock.calls.slice(1, 4)).toEqual([
      ['pc-1', expect.objectContaining({ message_limit: 100, before: 'cursor-1', open_id: expect.any(String), onTelemetry: expect.any(Function) })],
      ['pc-1', expect.objectContaining({ message_limit: 100, request_id: expect.any(String), onTelemetry: expect.any(Function) })],
      ['pc-1', expect.objectContaining({ message_limit: 100, before: 'cursor-2', open_id: expect.any(String), onTelemetry: expect.any(Function) })],
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

  it('preserves multiple loaded pages through Close refresh and stale-cursor recovery', async () => {
    const { api } = await import('../api');
    const activeClose = {
      attempt_id: 'close-active', phase: 'settling_active_work', outcome: null, run_ordinal: '1', run_status: 'running', failure: null, confirmation_snapshot: null,
      inspections: [], losses: [], residuals: [],
    } satisfies ProductConversationCloseView;
    const page = (id: string, ordinal: number, before: string | null, hasOlder: boolean) => makeSnapshot({
      close: activeClose,
      segments: [{ segment_ordinal: ordinal, transcript_row_id: `row-${id}`, slug: `row-${id}`, title: id, messages: [makeMessage(id, ordinal)], handoff: null }],
      before,
      has_older: hasOlder,
    });
    vi.mocked(api.getProductConversationHistorySnapshot)
      .mockResolvedValueOnce(makeSnapshot({ close: activeClose, before: 'cursor-1', has_older: true }))
      .mockResolvedValueOnce(page('older-1', 0, 'cursor-0', true))
      .mockResolvedValueOnce(page('older-0', -1, 'stale-cursor', true))
      .mockResolvedValueOnce(makeSnapshot({ close: { ...activeClose, phase: 'awaiting_retirement_inspection' }, before: 'fresh-cursor', has_older: true }))
      .mockRejectedValueOnce(new ApiResponseError('stale cursor', 400))
      .mockResolvedValueOnce(makeSnapshot({ close: { ...activeClose, phase: 'awaiting_retirement_inspection' }, before: 'retry-cursor', has_older: true }))
      .mockResolvedValueOnce(page('recovered', -2, null, false));

    renderPage();
    await waitForPageReady();
    fireEvent.click(screen.getByRole('button', { name: 'load older' }));
    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent('older-1'));
    fireEvent.click(screen.getByRole('button', { name: 'load older' }));
    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent('older-0'));
    act(() => notifyCloseSnapshotChanged('pc-1', 'stream'));
    await waitFor(() => expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(4));
    fireEvent.click(screen.getByRole('button', { name: 'load older' }));

    const expected = 'recovered,older-0,older-1,m-1,m-2,cont-1,m-3,m-4';
    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent(expected));
    const ids = screen.getByTestId('message-order').textContent?.split(',') ?? [];
    expect(new Set(ids).size).toBe(ids.length);
    act(() => emitLatestProjection({ messages: [makeMessage('m-3', 3, 'row-2'), makeMessage('m-4', 4, 'row-2')] as never }));
    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent(expected));
  });

  it('discards delayed older-page results after a new product route wins', async () => {
    const { api } = await import('../api');
    let resolveOlder!: (value: ProductConversationSnapshotView) => void;
    const older = new Promise<ProductConversationSnapshotView>((resolve) => { resolveOlder = resolve; });
    vi.mocked(api.getProductConversationHistorySnapshot)
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
    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent('beta-message'));
    await act(async () => resolveOlder(makeSnapshot({ segments: [{ segment_ordinal: 0, transcript_row_id: 'row-old', slug: 'old', title: 'Old A', messages: [makeMessage('stale-a', 1)], handoff: null }] })));

    expect(screen.getByTestId('message-order')).toHaveTextContent('beta-message');
    expect(screen.getByTestId('message-order')).toHaveTextContent('beta-message');
    expect(screen.getByTestId('message-order')).not.toHaveTextContent('stale-a');
  });

  it('discards delayed pagination errors and cleanup after the route owner changes', async () => {
    const { api } = await import('../api');
    let rejectOlder!: (reason: Error) => void;
    const older = new Promise<ProductConversationSnapshotView>((_resolve, reject) => { rejectOlder = reject; });
    vi.mocked(api.getProductConversationHistorySnapshot)
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
    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent('beta-message'));
    await act(async () => rejectOlder(new Error('stale A failure')));

    expect(screen.queryByText('stale A failure')).not.toBeInTheDocument();
    expect(conversationNavStackSpy.mock.lastCall?.[0]?.['loadingOlderMessages']).toBe(false);
  });

  it('ignores an old stream Close invalidation after a new product route wins', async () => {
    const { api } = await import('../api');
    const activeClose = {
      attempt_id: 'close-active',
      phase: 'settling_active_work', outcome: null, run_ordinal: '1', run_status: 'running', failure: null,
      confirmation_snapshot: null,
      inspections: [],
      losses: [],
      residuals: [],
    } satisfies ProductConversationCloseView;
    vi.mocked(api.getProductConversationHistorySnapshot)
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
    await waitFor(() => expect(screen.getByTestId('message-order')).toHaveTextContent('beta-message'));
    act(() => notifyCloseSnapshotChanged('row-2', 'stream'));
    await act(async () => Promise.resolve());

    expect(api.getProductConversationHistorySnapshot).toHaveBeenCalledTimes(2);
    expect(screen.getByTestId('message-order')).toHaveTextContent('beta-message');
    expect(screen.getByTestId('message-order')).toHaveTextContent('beta-message');
  });

  it('threads restore basis, root directory, system prompt, and explicit absent work scope', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot)
      .mockResolvedValueOnce(makeSnapshot({ work_identity: null }))
      .mockResolvedValueOnce(makeSnapshot({
        segments: [{ segment_ordinal: 0, transcript_row_id: 'row-old', slug: 'old', title: 'Old', messages: [makeMessage('old-message', 0)], handoff: null }],
        before: null,
        has_older: false,
      }));
    renderPage();
    await waitForPageReady();
    emitLatestProjection({ conversation: {} });
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
    expect(conversationNavStackSpy.mock.lastCall?.[0]?.['workScopeKey']).toBeUndefined();
  });

  it('keeps Recall collapsed with no Q&A load, subscription, timers, or autofocus side effects', async () => {
    const { api } = await import('../api');
    const setTimeoutSpy = vi.spyOn(window, 'setTimeout');
    const focusSpy = vi.spyOn(HTMLTextAreaElement.prototype, 'focus');

    renderPage();
    await waitForPageReady();

    expect(screen.getByRole('button', { name: 'Recall' })).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByTestId('chain-qa-column')).not.toBeInTheDocument();
    expect(api.getChain).not.toHaveBeenCalled();
    expect(chainStream.subscribe).not.toHaveBeenCalled();
    expect(setTimeoutSpy).not.toHaveBeenCalledWith(expect.any(Function), 300);
    expect(focusSpy).not.toHaveBeenCalled();

    setTimeoutSpy.mockRestore();
    focusSpy.mockRestore();
  });

  it('gates Recall actions until the deferred persisted chain hydrates, then submits exactly once', async () => {
    const { api } = await import('../api');
    let resolveChain: ((chain: ChainView) => void) | undefined;
    vi.mocked(api.getChain).mockImplementationOnce(() => new Promise((resolve) => { resolveChain = resolve; }));
    renderPage();
    await waitForPageReady();

    fireEvent.click(screen.getByRole('button', { name: 'Recall' }));
    expect(await screen.findByRole('status')).toHaveTextContent('Loading Recall…');
    expect(screen.queryByLabelText('recall draft')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Ask recall' })).not.toBeInTheDocument();
    expect(chainQaColumnSpy).not.toHaveBeenCalled();

    act(() => resolveChain?.(makeChain()));
    const draft = await screen.findByLabelText('recall draft');
    fireEvent.change(draft, { target: { value: 'Where did this land?' } });
    fireEvent.click(screen.getByRole('button', { name: 'Ask recall' }));
    await waitFor(() => expect(api.submitChainQuestion).toHaveBeenCalledTimes(1));
    expect(api.submitChainQuestion).toHaveBeenCalledWith('root-chain', 'Where did this land?');
  });

  it('focuses the hydrated Open Recall question once, but uses Close Recall for History', async () => {
    const { api } = await import('../api');
    let resolveOpenChain: ((chain: ChainView) => void) | undefined;
    vi.mocked(api.getChain).mockImplementationOnce(() => new Promise((resolve) => { resolveOpenChain = resolve; }));
    renderPage();
    await waitForPageReady();

    const trigger = screen.getByRole('button', { name: 'Recall' });
    trigger.focus();
    fireEvent.click(trigger);
    expect(trigger).toHaveFocus();
    expect(screen.queryByLabelText('recall draft')).not.toBeInTheDocument();
    act(() => resolveOpenChain?.(makeChain()));
    expect(await screen.findByLabelText('recall draft')).toHaveFocus();

    fireEvent.click(screen.getByRole('button', { name: 'Close Recall' }));
    expect(trigger).toHaveFocus();
    fireEvent.click(trigger);
    expect(await screen.findByLabelText('recall draft')).toHaveFocus();

    cleanup();
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(makeSnapshot({
      ordinary_lifecycle: 'history',
      writable_transcript_row_id: null,
    }));
    renderPage();
    await waitForPageReady();
    fireEvent.click(screen.getByRole('button', { name: 'Recall' }));
    expect(await screen.findByRole('button', { name: 'Close Recall' })).toHaveFocus();
    expect(screen.getByLabelText('recall draft')).toBeDisabled();
  });

  it('lazily loads persisted Recall and subscribes until close, then restores trigger focus', async () => {
    const { api } = await import('../api');
    renderPage();
    await waitForPageReady();

    const trigger = screen.getByRole('button', { name: 'Recall' });
    fireEvent.click(trigger);

    expect(await screen.findByTestId('chain-qa-column')).toBeInTheDocument();
    expect(await screen.findByLabelText('recall draft')).toHaveFocus();
    await waitFor(() => expect(api.getChain).toHaveBeenCalledWith('root-chain'));
    await waitFor(() => expect(screen.getByTestId('chain-qa-persisted-count')).toHaveTextContent('1'));
    expect(chainStream.subscribe).toHaveBeenCalledWith('root-chain', expect.any(Function), expect.any(Function));
    expect(screen.getByTestId('product-conversation-composer')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Close Recall' }));
    expect(screen.queryByTestId('chain-qa-column')).not.toBeInTheDocument();
    expect(chainStream.close).toHaveBeenCalledTimes(1);
    expect(trigger).toHaveFocus();
  });

  it('closes Recall with Escape, tears down streaming, and restores trigger focus', async () => {
    renderPage();
    await waitForPageReady();
    const trigger = screen.getByRole('button', { name: 'Recall' });
    fireEvent.click(trigger);
    await screen.findByTestId('chain-qa-column');

    fireEvent.keyDown(window, { key: 'Escape' });

    expect(screen.queryByTestId('chain-qa-column')).not.toBeInTheDocument();
    expect(chainStream.close).toHaveBeenCalledTimes(1);
    expect(trigger).toHaveFocus();
  });

  it('streams Recall through the existing chain reducer and refetches persisted completion', async () => {
    const { api } = await import('../api');
    renderPage();
    await waitForPageReady();
    fireEvent.click(screen.getByRole('button', { name: 'Recall' }));
    await screen.findByTestId('chain-qa-column');

    fireEvent.change(screen.getByLabelText('recall draft'), { target: { value: 'Where did this land?' } });
    fireEvent.click(screen.getByRole('button', { name: 'Ask recall' }));
    await waitFor(() => expect(api.submitChainQuestion).toHaveBeenCalledWith('root-chain', 'Where did this land?'));

    const handleEvent = chainStream.subscribe.mock.calls.at(-1)?.[1];
    expect(handleEvent).toEqual(expect.any(Function));
    act(() => handleEvent?.({ type: 'chain_qa_token', chain_qa_id: 'qa-new', delta: 'unfinished narration' }));
    expect(screen.getByTestId('chain-qa-inflight')).toHaveTextContent('unfinished narration');
    act(() => handleEvent?.({ type: 'chain_qa_answer_reset', chain_qa_id: 'qa-new' }));
    expect(screen.getByTestId('chain-qa-inflight')).not.toHaveTextContent('unfinished narration');
    act(() => handleEvent?.({ type: 'chain_qa_token', chain_qa_id: 'qa-new', delta: 'In main' }));
    expect(screen.getByTestId('chain-qa-inflight')).toHaveTextContent('qa-new:In main:');

    act(() => handleEvent?.({ type: 'chain_qa_completed', chain_qa_id: 'qa-new', full_answer: 'In main' }));
    await waitFor(() => expect(api.getChain).toHaveBeenCalledTimes(2));
    expect(screen.getByTestId('chain-qa-inflight')).toHaveTextContent('');
  });

  it('surfaces failed streams and retry through the existing authoritative reducer path', async () => {
    renderPage();
    await waitForPageReady();
    fireEvent.click(screen.getByRole('button', { name: 'Recall' }));
    await screen.findByTestId('chain-qa-column');
    fireEvent.change(screen.getByLabelText('recall draft'), { target: { value: 'What failed?' } });
    fireEvent.click(screen.getByRole('button', { name: 'Ask recall' }));
    await waitFor(() => expect(screen.getByTestId('chain-qa-inflight')).toHaveTextContent('qa-new'));

    const handleEvent = chainStream.subscribe.mock.calls.at(-1)?.[1];
    const handleError = chainStream.subscribe.mock.calls.at(-1)?.[2];
    act(() => handleError?.(new Event('error')));
    expect(screen.getByTestId('chain-qa-sse-lost')).toHaveTextContent('true');

    act(() => handleEvent?.({
      type: 'chain_qa_failed',
      chain_qa_id: 'qa-new',
      error: 'stream stopped',
      partial_answer: 'Partial',
    }));
    await waitFor(() => expect(screen.getByTestId('chain-qa-inflight')).toHaveTextContent(''));

    fireEvent.click(screen.getByRole('button', { name: 'Retry recall connection' }));
    expect(screen.getByTestId('chain-qa-sse-lost')).toHaveTextContent('false');
  });

  it('keeps failed submissions editable for retry', async () => {
    const { api } = await import('../api');
    vi.mocked(api.submitChainQuestion).mockRejectedValueOnce(new Error('submit failed'));
    renderPage();
    await waitForPageReady();
    fireEvent.click(screen.getByRole('button', { name: 'Recall' }));
    await screen.findByTestId('chain-qa-column');

    fireEvent.change(screen.getByLabelText('recall draft'), { target: { value: 'Try again' } });
    fireEvent.click(screen.getByRole('button', { name: 'Ask recall' }));

    await waitFor(() => expect(screen.getByLabelText('recall draft')).toHaveValue('Try again'));
    expect(screen.getByRole('alert')).toHaveTextContent('submit failed');

    fireEvent.click(screen.getByRole('button', { name: 'Ask recall' }));
    await waitFor(() => expect(api.submitChainQuestion).toHaveBeenCalledTimes(2));
    expect(api.submitChainQuestion).toHaveBeenLastCalledWith('root-chain', 'Try again');
  });

  it('keeps History Recall persisted but disables asking and re-ask mutations', async () => {
    const { api } = await import('../api');
    vi.mocked(api.getProductConversationHistorySnapshot).mockResolvedValueOnce(makeSnapshot({
      ordinary_lifecycle: 'history',
      writable_transcript_row_id: null,
    }));
    renderPage();
    await waitForPageReady();
    fireEvent.click(screen.getByRole('button', { name: 'Recall' }));

    expect(await screen.findByTestId('chain-qa-persisted-count')).toHaveTextContent('1');
    expect(screen.getByLabelText('recall draft')).toBeDisabled();
    expect(chainQaColumnSpy.mock.lastCall?.[0]?.['onSubmit']).toBeUndefined();
    fireEvent.click(screen.getByRole('button', { name: 'Re-ask recall' }));
    expect(screen.getByLabelText('recall draft')).toHaveValue('');
  });

  it('persists Recall drafts by ProductConversation identity across close and unmount', async () => {
    const first = renderPage();
    await waitForPageReady();
    fireEvent.click(screen.getByRole('button', { name: 'Recall' }));
    fireEvent.change(await screen.findByLabelText('recall draft'), { target: { value: 'durable aggregate draft' } });
    fireEvent.click(screen.getByRole('button', { name: 'Close Recall' }));
    expect(localStorage.getItem('phoenix:product-conversation-draft:pc-1')).toBe('durable aggregate draft');

    fireEvent.click(screen.getByRole('button', { name: 'Recall' }));
    expect(await screen.findByLabelText('recall draft')).toHaveValue('durable aggregate draft');
    first.unmount();

    renderPage();
    await waitForPageReady();
    fireEvent.click(screen.getByRole('button', { name: 'Recall' }));
    expect(await screen.findByLabelText('recall draft')).toHaveValue('durable aggregate draft');
    expect(localStorage.getItem('phoenix:chain-draft:root-chain')).toBeNull();
  });

  it('collapses Recall and isolates chain state when a new ProductConversation route wins', async () => {
    const { api } = await import('../api');
    let resolveOldChain: ((chain: ChainView) => void) | undefined;
    vi.mocked(api.getChain)
      .mockImplementationOnce(() => new Promise((resolve) => { resolveOldChain = resolve; }))
      .mockResolvedValueOnce(makeChain({ root_conv_id: 'root-chain-2', display_name: 'Product Beta' }));
    vi.mocked(api.getProductConversationHistorySnapshot)
      .mockResolvedValueOnce(makeSnapshot())
      .mockResolvedValueOnce(makeSnapshot({
        product_conversation_id: 'pc-2',
        canonical_route: '/product-conversations/pc-2',
        presentation: { kind: 'state', display_name: 'Product Beta', presentation_mode: 'idle' },
        chain_qa_compatibility: { root_transcript_row_id: 'root-chain-2', url: '/chains/root-chain-2' },
      }));

    renderPage(undefined, true);
    await waitForPageReady();
    fireEvent.click(screen.getByRole('button', { name: 'Recall' }));
    await waitFor(() => expect(api.getChain).toHaveBeenCalledWith('root-chain'));
    fireEvent.click(screen.getByRole('button', { name: 'open second product' }));

    expect(await screen.findByRole('heading', { name: 'Product Beta' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Recall' })).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByTestId('chain-qa-column')).not.toBeInTheDocument();
    expect(chainStream.close).toHaveBeenCalledTimes(1);

    act(() => resolveOldChain?.(makeChain({ display_name: 'Stale Alpha' })));
    fireEvent.click(screen.getByRole('button', { name: 'Recall' }));
    await waitFor(() => expect(api.getChain).toHaveBeenCalledWith('root-chain-2'));
    expect(screen.getByTestId('chain-qa-root')).toHaveTextContent('root-chain-2');
  });

  it('keeps successful pagination visually silent and ignores malformed message hashes', async () => {
    renderPage('/product-conversations/pc-1#message-%');
    await waitForPageReady();
    expect(screen.queryByText(/Earlier history available|Complete snapshot loaded|Loading earlier history/)).not.toBeInTheDocument();
    expect(screen.getByTestId('message-order')).toHaveTextContent('m-1,m-2');
  });

});
