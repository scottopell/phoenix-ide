import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { forwardRef, memo, Profiler, StrictMode, useEffect, useImperativeHandle } from 'react';
import { MemoryRouter, Route, Routes, useNavigate } from 'react-router-dom';
import { ProductConversationPage } from './ProductConversationPage';
import type { EmbeddedConversationProjection } from './ConversationPage';
import type { PendingUserMessage } from '../hooks/useMessageQueue';
import { ConversationContext } from '../conversation/ConversationContext';
import { ConversationStore } from '../conversation/ConversationStore';
import { ConversationReadinessProvider } from '../contexts/ConversationReadinessContext';
import { ViewerSlotProvider } from '../contexts/ViewerSlotContext';
import { FileExplorerProvider } from '../components/FileExplorer';
import { FocusScopeProvider } from '../hooks/useFocusScope';
import { getProductConversationScenario } from '../fixtures/productConversation/scenarios';
import { api, type Conversation, type Message, type ProductConversationHistorySnapshotView as ProductConversationSnapshotView } from '../api';
import type { RenderUnit } from '../conversation/renderUnits';
import type { EnrichedMessage } from '../generated/EnrichedMessage';
import type { VirtualTranscriptHandle, VirtualTranscriptProps } from '../components/VirtualTranscript';
import { notifyProductConversationSnapshotChanged } from '../notifications';
import { useConnection } from '../hooks/useConnection';
import type { InitPayload } from '../conversation/atom';

const probe = vi.hoisted(() => ({
  structuralRenders: 0,
  embeddedRenders: 0,
  historyRenders: 0,
  projection: null as EmbeddedConversationProjection | null,
  followAllowed: true,
  scrollToTail: vi.fn(),
  pinned: undefined as ((pinned: boolean) => void) | undefined,
  totalExtentChanged: undefined as VirtualTranscriptProps<RenderUnit>['onTotalExtentChange'],
}));

// ProductConversationPage, ConversationNavStack, MessageList and StreamingMessage
// are real; the executor and heavy display children are test doubles. These
// are commit/ownership assertions, not browser geometry conformance.
vi.mock('../components/VirtualTranscript', async () => {
  const actual = await vi.importActual<typeof import('../components/VirtualTranscript')>('../components/VirtualTranscript');
  return {
    ...actual,
    VirtualTranscript: forwardRef<VirtualTranscriptHandle, VirtualTranscriptProps<RenderUnit>>(function Transcript(props, ref) {
      probe.structuralRenders++;
      probe.pinned = props.onPinnedChange;
      probe.totalExtentChanged = props.onTotalExtentChange;
      useImperativeHandle(ref, () => ({
        scrollToTail: probe.scrollToTail,
        setTailFollowAllowed: (allowed) => { probe.followAllowed = allowed; },
        scrollToIndex: () => {},
        scrollToOwnedIndex: () => {},
        cancelPosition: () => {},
        scrollBy: () => {},
        isProgrammaticScroll: () => false,
        captureVisibleAnchor: () => null,
        preserveViewportOnNextItemsChange: () => {},
        measureOffsetForIndex: () => null,
        measureOffsetForIndexAtSnapshot: () => null,
        layoutRevision: () => 1,
        physicalSnapshot: () => ({ visibleRange: null, renderedRange: null, viewportTop: 0, layoutRevision: 1 }),
      }), []);
      return <div id="messages" ref={props.scrollerRef}>
        {props.items.map((unit, index) => <div key={props.getKey(unit, index)} data-owned-unit={unit.key} data-kind={unit.kind}>
          {props.renderItem(unit, index)}
        </div>)}
      </div>;
    }),
  };
});
vi.mock('./ConversationPage', () => ({
  EmbeddedConversationPage: ({ onProjectionChange }: {
    onProjectionChange?: (projection: EmbeddedConversationProjection | null) => void;
  }) => {
    probe.embeddedRenders++;
    useEffect(() => {
      // Deliberately retain the passive projection even after the atom acknowledges a send.
      if (probe.projection) onProjectionChange?.(probe.projection);
    }, [onProjectionChange]);
    return <div>composer owner</div>;
  },
}));
vi.mock('../hooks/useConnection', () => ({ useConnection: vi.fn() }));

vi.mock('../components/MessageComponents', () => ({
  UserMessage: ({ message }: { message: Message }) => <div>{(message.content as { text: string }).text}</div>,
  QueuedUserMessage: ({ message }: { message: PendingUserMessage }) => <div>{message.text}</div>,
  AgentMessage: memo(({ message }: { message: Message }) => {
    probe.historyRenders++;
    return <div>{(message.content as Array<{ text?: string }>).map((block) => block.text).join('')}</div>;
  }),
  SubAgentStatus: () => null,
  formatMessageTime: () => '12:00',
}));
vi.mock('../components/MessageContextMenu', () => ({ MessageContextMenu: () => null, OPEN_MESSAGE_VIEWER_EVENT: 'phoenix:open-message-viewer' }));
vi.mock('../components/AutomaticContinuationControl', () => ({ AutomaticContinuationControl: () => null }));
vi.mock('../components/InlineMessageReaction', () => ({ InlineMessageReaction: () => null }));
vi.mock('react-markdown', () => ({ default: ({ children }: { children: string }) => <span>{children}</span> }));
vi.mock('../utils/syntaxHighlighter', () => ({ SyntaxHighlighter: () => null, oneDark: {}, oneLight: {} }));
vi.mock('../api', async () => {
  const actual = await vi.importActual<typeof import('../api')>('../api');
  return { ...actual, api: { ...actual.api,
    getProductConversationHistorySnapshot: vi.fn(),
    reportProductConversationOpen: vi.fn().mockResolvedValue(undefined),
  } };
});

let store: ConversationStore;
let snapshot: ProductConversationSnapshotView;
let latestRow: string;
const commits: Array<Array<{ key: string; kind: string; text: string }>> = [];

function message(id: string, row: string, sequenceId: number, type: 'user' | 'agent', text: string): Message {
  return { message_id: id, conversation_id: row, sequence_id: sequenceId, message_type: type,
    content: type === 'agent' ? [{ type: 'text', text }] : { text }, created_at: '2026-01-01T00:00:00Z' };
}
function seed(row: string) {
  store.dispatch(row, { type: 'set_initial_data', conversationId: row,
    conversation: { id: row, slug: row, updated_at: '2026-01-01', state: { type: 'llm_requesting', attempt: 1 } } as Conversation,
    messages: [message(`${row}-user`, row, 1, 'user', 'Live question')],
    phase: { type: 'llm_requesting', attempt: 1 }, contextWindow: { used: 0 } });
}
function token(requestId: string, delta: string, row = latestRow) {
  store.dispatch(row, { type: 'sse_token', sequenceId: store.getSnapshot(row).lastAppliedEventSeq + 1, requestId, delta });
}
function captureCommit() {
  commits.push(Array.from(document.querySelectorAll<HTMLElement>('[data-owned-unit]')).map((node) => ({
    key: node.dataset['ownedUnit']!, kind: node.dataset['kind']!, text: node.textContent ?? '',
  })));
}
function NavigationProbe() {
  const navigate = useNavigate();
  return <><button onClick={() => navigate('/product-conversations/other')}>leave A</button>
    <button onClick={() => navigate('/product-conversations/pc-product-alpha')}>return A</button></>;
}
async function open(expected = 'Live question') {
  const view = render(<StrictMode><ConversationContext.Provider value={store}>
    <MemoryRouter initialEntries={['/product-conversations/pc-product-alpha']}>
    <NavigationProbe />
    <ConversationReadinessProvider><ViewerSlotProvider browserSessionActive={false}>
      <FileExplorerProvider><FocusScopeProvider>
        <Profiler id="product" onRender={captureCommit}><Routes>
          <Route path="/product-conversations/:productConversationId" element={<ProductConversationPage />} />
        </Routes></Profiler>
      </FocusScopeProvider></FileExplorerProvider>
    </ViewerSlotProvider></ConversationReadinessProvider>
    </MemoryRouter>
  </ConversationContext.Provider></StrictMode>);
  await screen.findByText(expected);
  return view;
}
async function startStream(requestId = 'ordinary-final', text = 'RESPONSE_MARKER') {
  act(() => token(requestId, text));
  await screen.findByText(text);
  commits.length = 0;
}
function responseRows(commit: typeof commits[number], requestId = 'ordinary-final') {
  return commit.filter((row) => row.key === requestId);
}

beforeEach(() => {
  store = new ConversationStore();
  probe.structuralRenders = 0;
  probe.embeddedRenders = 0;
  probe.historyRenders = 0;
  probe.projection = null;
  probe.followAllowed = true;
  probe.scrollToTail.mockClear();
  commits.length = 0;
  localStorage.clear();
  const scenario = getProductConversationScenario('desktop-multi-segment-qa-work');
  const fixture = scenario.snapshot!;
  latestRow = fixture.latest_transcript_row_id;
  snapshot = { ...fixture, chain_qa_compatibility: null, writable_transcript_row_id: latestRow,
    segments: fixture.segments.map((segment) => ({ ...segment,
      messages: segment.transcript_row_id === latestRow ? [] : segment.messages })) };
  vi.mocked(api.getProductConversationHistorySnapshot).mockImplementation(async () => snapshot);
  seed(latestRow);
  vi.spyOn(HTMLElement.prototype, 'scrollHeight', 'get').mockReturnValue(1000);
  vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockReturnValue(400);
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

describe('ProductConversationPage atom-owned ordinary finalization', () => {
  it.each(['local', 'target-scoped'] as const)('acknowledges a %s pending send in every commit despite a stale passive projection', async (identity) => {
    const localId = 'pending-send';
    const acknowledgedId = identity === 'local' ? localId : `${latestRow}:${localId}`;
    const pendingMessages: PendingUserMessage[] = [
      { localId, text: 'PENDING_SEND_MARKER', images: [], status: 'pending' },
      { localId: 'still-pending', text: 'UNACKNOWLEDGED_MARKER', images: [], status: 'pending' },
    ];
    probe.projection = {
      slug: latestRow, conversationId: latestRow, conversation: store.getSnapshot(latestRow).conversation,
      pendingMessages, isArchived: false, serverArchived: false,
      onRetryPending: vi.fn(), onCancelSteering: vi.fn(), onOpenFile: vi.fn(), filePathRootDir: '/',
    };
    await open();
    await waitFor(() => expect(document.querySelector(`[data-owned-unit="${localId}"]`)).toHaveTextContent('PENDING_SEND_MARKER'));
    const pendingRow = document.querySelector<HTMLElement>(`[data-owned-unit="${localId}"]`)!;
    expect(pendingRow.dataset['kind']).toBe('pending_user');
    commits.length = 0;

    act(() => store.dispatch(latestRow, {
      type: 'sse_message', sequenceId: store.getSnapshot(latestRow).lastAppliedEventSeq + 1,
      message: message(acknowledgedId, latestRow, 2, 'user', 'ACKNOWLEDGED_SEND_MARKER'),
    }));

    expect(probe.projection.pendingMessages).toBe(pendingMessages);
    expect(probe.projection.pendingMessages).toHaveLength(2);
    expect(commits.length).toBeGreaterThan(0);
    for (const commit of commits) {
      const rows = commit.filter((row) => row.key === localId || row.key === acknowledgedId);
      expect(rows).toHaveLength(1);
      expect(rows[0]).toEqual({ key: acknowledgedId, kind: 'user', text: 'ACKNOWLEDGED_SEND_MARKER' });
      expect(commit.find((row) => row.key === 'still-pending')?.kind).toBe('pending_user');
      expect(new Set(commit.map((row) => row.key)).size).toBe(commit.length);
    }
    expect(document.getElementById('messages')).not.toHaveTextContent('PENDING_SEND_MARKER');
    expect(document.querySelector('[data-owned-unit="still-pending"]')).toHaveTextContent('UNACKNOWLEDGED_MARKER');
    if (identity === 'local') {
      expect(document.querySelector(`[data-owned-unit="${localId}"]`)).toBe(pendingRow);
    }
  });

  it.each(['following', 'reading'] as const)('never commits a missing or emptied response while %s', async (mode) => {
    await open();
    await startStream();
    const scroller = document.getElementById('messages')!;
    scroller.scrollTop = mode === 'reading' ? 350 : 600;
    act(() => probe.totalExtentChanged?.(1000));
    if (mode === 'reading') {
      fireEvent.wheel(scroller, { deltaY: -100 });
      fireEvent.scroll(scroller);
      act(() => probe.pinned?.(false));
      expect(probe.followAllowed).toBe(false);
    } else {
      act(() => probe.pinned?.(true));
      expect(probe.followAllowed).toBe(true);
    }
    commits.length = 0;
    probe.scrollToTail.mockClear();
    act(() => store.dispatch(latestRow, {
      type: 'sse_message', sequenceId: store.getSnapshot(latestRow).lastAppliedEventSeq + 1,
      message: message('ordinary-final', latestRow, 2, 'agent', 'RESPONSE_MARKER finalized'),
    }));
    expect(commits.length).toBeGreaterThan(0);
    for (const commit of commits) {
      const rows = responseRows(commit);
      expect(rows).toHaveLength(1);
      expect(rows[0]!.text).toContain('RESPONSE_MARKER');
      expect(commit.some((row) => row.key === 'root-1')).toBe(true);
    }
    expect(responseRows(commits.at(-1)!)[0]!.kind).toBe('agent_turn');
    expect(screen.getByText('RESPONSE_MARKER finalized')).toBeInTheDocument();
    if (mode === 'reading') {
      expect(probe.followAllowed).toBe(false);
      expect(probe.scrollToTail).not.toHaveBeenCalled();
    } else expect(probe.followAllowed).toBe(true);
  });

  it('keeps token/heartbeat churn at the leaf, not the product owner or history', async () => {
    await open();
    await startStream();
    const baseline = { structural: probe.structuralRenders, embedded: probe.embeddedRenders, history: probe.historyRenders };
    for (let index = 0; index < 5; index++) act(() => {
      token('ordinary-final', ` token-${index}`);
      store.dispatch(latestRow, { type: 'sse_event_observed' });
    });
    await screen.findByText('RESPONSE_MARKER token-0 token-1 token-2 token-3 token-4');
    expect(probe.structuralRenders).toBe(baseline.structural);
    expect(probe.embeddedRenders).toBe(baseline.embedded);
    expect(probe.historyRenders).toBe(baseline.history);
    expect(commits.length).toBeGreaterThan(0);
  });

  it('scopes a replacement request and clears terminal streaming without retaining stale content', async () => {
    await open();
    await startStream();
    act(() => token('next-request', 'NEXT_REQUEST_MARKER'));
    await screen.findByText('NEXT_REQUEST_MARKER');
    expect(document.querySelector('[data-owned-unit="ordinary-final"]')).toBeNull();
    expect(screen.queryByText('RESPONSE_MARKER')).not.toBeInTheDocument();
    act(() => store.dispatch(latestRow, { type: 'sse_agent_done', sequenceId: store.getSnapshot(latestRow).lastAppliedEventSeq + 1 }));
    expect(document.querySelector('[data-owned-unit="next-request"]')).toBeNull();
    expect(screen.queryByText('NEXT_REQUEST_MARKER')).not.toBeInTheDocument();
    expect(screen.getByText('Live question')).toBeInTheDocument();
  });

  it.each([
    ['fresh init', 'in-place'], ['failed reconnect', 'in-place'],
    ['fresh init', 'A→B→A'], ['fresh init', 'remount'],
  ] as const)('keeps snapshot authority through %s / %s', async (reconnect, lifecycle) => {
    let view = await open();
    expect(screen.getByText('composer owner')).toBeInTheDocument();
    const staleMessage = store.getSnapshot(latestRow).messages[0]!;
    expect(staleMessage.content).toEqual({ text: 'Live question' });
    const snapshotMessage: EnrichedMessage = { ...staleMessage,
      origin: staleMessage.origin ?? { kind: 'unknown_historical' },
      display_data: staleMessage.display_data ?? null, usage_data: staleMessage.usage_data ?? null };

    snapshot = { ...snapshot, ordinary_lifecycle: 'open', writable_transcript_row_id: null,
      close: { attempt_id: 'incomplete-close', phase: 'completed', outcome: 'close_incomplete',
        run_ordinal: '1', run_status: 'stopped', confirmation_snapshot: null,
        inspections: [], losses: [], residuals: [], failure: null },
      segments: snapshot.segments.map((segment) => segment.transcript_row_id === latestRow
        ? { ...segment, messages: [{ ...snapshotMessage, content: { text: 'FRESH_SNAPSHOT_MARKER' } }] }
        : segment) };
    act(() => notifyProductConversationSnapshotChanged('pc-product-alpha'));

    await screen.findByTestId('product-conversation-close-incomplete');
    expect(screen.queryByText('composer owner')).not.toBeInTheDocument();
    expect(screen.getByTestId('product-conversation-close-live-update-owner')).toBeInTheDocument();
    expect(screen.getByText('FRESH_SNAPSHOT_MARKER')).toBeInTheDocument();
    expect(screen.queryByText('Live question')).not.toBeInTheDocument();
    expect(store.getSnapshot(latestRow).messages[0]).toBe(staleMessage);

    const renders = probe.structuralRenders;
    commits.length = 0;
    act(() => store.dispatch(latestRow, {
      type: 'sse_message', sequenceId: store.getSnapshot(latestRow).lastAppliedEventSeq + 1,
      message: { ...staleMessage, message_id: 'retired-atom-arrival', content: { text: 'RETIRED_ATOM_MARKER' } },
    }));
    expect(store.getSnapshot(latestRow).messages.at(-1)!.content).toEqual({ text: 'RETIRED_ATOM_MARKER' });
    expect(probe.structuralRenders).toBe(renders);
    expect(commits).toHaveLength(0);
    expect(screen.queryByText('RETIRED_ATOM_MARKER')).not.toBeInTheDocument();

    snapshot = { ...snapshot, segments: snapshot.segments.map((segment) => segment.transcript_row_id === latestRow
      ? { ...segment, messages: [{ ...snapshotMessage, content: { text: 'REFRESHED_SNAPSHOT_MARKER' } }] }
      : segment) };
    act(() => notifyProductConversationSnapshotChanged('pc-product-alpha'));
    await screen.findByText('REFRESHED_SNAPSHOT_MARKER');
    expect(screen.queryByText('RETIRED_ATOM_MARKER')).not.toBeInTheDocument();
    expect(screen.queryByText('FRESH_SNAPSHOT_MARKER')).not.toBeInTheDocument();

    const retryClose = snapshot.close!;
    if (lifecycle === 'A→B→A') {
      const snapshotA = snapshot;
      snapshot = { ...snapshot, product_conversation_id: 'other', latest_transcript_row_id: 'row-B',
        close: null, writable_transcript_row_id: 'row-B', segments: [] };
      seed('row-B');
      fireEvent.click(screen.getByText('leave A'));
      await screen.findByText('Live question');
      snapshot = { ...snapshotA, close: null };
      fireEvent.click(screen.getByText('return A'));
      await screen.findByText('REFRESHED_SNAPSHOT_MARKER');
    } else if (lifecycle === 'remount') {
      view.unmount();
      snapshot = { ...snapshot, close: null };
      view = await open('REFRESHED_SNAPSHOT_MARKER');
    }
    expect(screen.queryByText('Live question')).not.toBeInTheDocument();
    expect(screen.queryByText('RETIRED_ATOM_MARKER')).not.toBeInTheDocument();

    const connectionCalls = vi.mocked(useConnection).mock.calls.length;
    commits.length = 0;
    snapshot = { ...snapshot, close: { ...retryClose, phase: 'settling_active_work',
      outcome: null, run_ordinal: '2', run_status: 'running' } };
    act(() => notifyProductConversationSnapshotChanged('pc-product-alpha'));
    await waitFor(() => expect(vi.mocked(useConnection).mock.calls.length).toBeGreaterThan(connectionCalls));
    const hydration = vi.mocked(useConnection).mock.calls.at(-1)![0];
    expect(hydration.conversationId).toBe(latestRow);
    expect(hydration.getLastAppliedEventSeq).toBeUndefined();
    expect(hydration.getTranscriptGeneration).toBeUndefined();
    expect(screen.queryByTestId('product-conversation-close-incomplete')).not.toBeInTheDocument();
    expect(screen.queryByText('composer owner')).not.toBeInTheDocument();
    expect(commits.length).toBeGreaterThan(0);
    for (const commit of commits) {
      expect(commit.find((row) => row.key === staleMessage.message_id)?.text).toBe('REFRESHED_SNAPSHOT_MARKER');
      expect(commit.some((row) => row.text.includes('RETIRED_ATOM_MARKER'))).toBe(false);
    }

    const connectionId = Symbol('current-hydration');
    const previousConnectionId = Symbol('previous-hydration');
    act(() => {
      hydration.dispatch({ type: 'connection_reset' });
      hydration.dispatch({ type: 'connection_opened', epoch: 1, connectionId });
      hydration.dispatch({ type: 'connection_state', state: 'live', epoch: 1 });
    });
    expect(screen.getByText('REFRESHED_SNAPSHOT_MARKER')).toBeInTheDocument();
    expect(screen.queryByText('Live question')).not.toBeInTheDocument();
    const payload: InitPayload = {
      conversation: store.getSnapshot(latestRow).conversation!,
      messages: [{ ...staleMessage, content: { text: 'FRESH_INIT_MARKER' } }],
      steeringMessages: [], phase: { type: 'llm_requesting', attempt: 1 }, contextWindow: { used: 0 },
      transcriptGeneration: 1, streamIncarnation: 'retry-stream',
      lastAppliedEventSeq: 10, pendingAnchorSequenceId: 10, pendingEvents: [], pendingTruncated: false,
      transcriptCoverage: 'complete',
    };
    act(() => hydration.dispatch({ type: 'sse_init', epoch: 0, connectionId, payload }));
    act(() => hydration.dispatch({ type: 'sse_init', epoch: 1, connectionId: previousConnectionId, payload }));
    expect(store.getSnapshot(latestRow).messages[0]).toBe(staleMessage);
    expect(screen.getByText('REFRESHED_SNAPSHOT_MARKER')).toBeInTheDocument();

    if (reconnect === 'failed reconnect') {
      act(() => {
        hydration.dispatch({ type: 'connection_state', state: 'failed', epoch: 1 });
        hydration.dispatch({ type: 'sse_error', epoch: 1,
          error: { type: 'ConnectionFailed', retriesExhausted: true } });
      });
      expect(screen.getByText('REFRESHED_SNAPSHOT_MARKER')).toBeInTheDocument();
      expect(screen.queryByText('composer owner')).not.toBeInTheDocument();
      expect(screen.queryByText('FRESH_INIT_MARKER')).not.toBeInTheDocument();
      return;
    }
    // A preserve-only init cannot prove the cached transcript was refreshed.
    act(() => hydration.dispatch({ type: 'sse_init', epoch: 1, connectionId,
      payload: { ...payload, messages: [], transcriptCoverage: 'preserve' } }));
    expect(screen.getByText('REFRESHED_SNAPSHOT_MARKER')).toBeInTheDocument();
    commits.length = 0;
    act(() => hydration.dispatch({ type: 'sse_init', epoch: 1, connectionId, payload }));
    expect(screen.getByText('FRESH_INIT_MARKER')).toBeInTheDocument();
    expect(screen.getByText('composer owner')).toBeInTheDocument();
    expect(screen.queryByText('REFRESHED_SNAPSHOT_MARKER')).not.toBeInTheDocument();
    for (const commit of commits) {
      expect(commit.find((row) => row.key === staleMessage.message_id)?.text).toBe('FRESH_INIT_MARKER');
    }
    snapshot = { ...snapshot, close: null, writable_transcript_row_id: latestRow };
    await act(async () => notifyProductConversationSnapshotChanged('pc-product-alpha'));
    expect(store.getTranscriptAuthority(latestRow).kind).toBe('live');
    await startStream('restored-stream', 'RESTORED_STREAM_MARKER');
    expect(screen.getByText('RESTORED_STREAM_MARKER')).toBeInTheDocument();
  });

  it('switches the atom subscription with the latest member and ignores the previous member', async () => {
    await open();
    await startStream();
    seed('row-successor');
    snapshot = { ...snapshot, latest_transcript_row_id: 'row-successor', writable_transcript_row_id: 'row-successor',
      segments: [...snapshot.segments, { ...snapshot.segments.at(-1)!, transcript_row_id: 'row-successor',
        slug: 'row-successor', segment_ordinal: 4, messages: [] }] };
    act(() => notifyProductConversationSnapshotChanged('pc-product-alpha'));
    await waitFor(() => expect(document.querySelector('[data-owned-unit="ordinary-final"]')).toBeNull());
    act(() => token('successor-request', 'SUCCESSOR_MARKER', 'row-successor'));
    await screen.findByText('SUCCESSOR_MARKER');
    const baseline = probe.embeddedRenders;
    act(() => token('ordinary-final', 'STALE_MEMBER_MARKER'));
    expect(probe.embeddedRenders).toBe(baseline);
    expect(screen.queryByText(/STALE_MEMBER_MARKER/)).not.toBeInTheDocument();
    expect(document.querySelector('[data-owned-unit="successor-request"]')).not.toBeNull();
  });
});
