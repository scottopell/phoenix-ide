import { describe, it, expect, vi, afterEach } from 'vitest';
import { useEffect, useRef } from 'react';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import { ConversationPage } from './ConversationPage';
import { resolveOwnedConversationTarget } from '../conversation/conversationNavigation';
import { DesktopLayout } from '../components/DesktopLayout';
import { ConversationContext } from '../conversation/ConversationContext';
import { DraftContext } from '../conversation/DraftContext';
import { ConversationStore, type InitPayload, type SSEAction } from '../conversation';
import { DraftStore } from '../conversation/DraftStore';
import { api, ExpansionError, type Conversation, type Message } from '../api';
import { ConversationReadinessProvider } from '../contexts/ConversationReadinessContext';
import { cacheDB } from '../cache';

const viewportFlags = vi.hoisted(() => ({ isDesktop: true, isWideDesktop: true }));
const navStackProps = vi.hoisted(() => ({ onOpenCommissionReview: undefined as ((sequenceId: number) => void) | undefined }));

vi.mock('../api', async () => {
  const actual = await vi.importActual<typeof import('../api')>('../api');
  return {
    ...actual,
    api: {
      ...actual.api,
      getConversation: vi.fn(),
      getConversationBySlug: vi.fn(),
      getConversationRoute: vi.fn(),
      getConversationRouteBySlug: vi.fn(),
      getConversationMessagesBefore: vi.fn(),
      reconcileAcceptedMessages: vi.fn(),
      listConversations: vi.fn(() => Promise.resolve([])),
      listArchivedConversations: vi.fn(() => Promise.resolve([])),
      getModels: vi.fn(() => Promise.resolve([])),
      getPrStatus: vi.fn(() => Promise.resolve({ found: false })),
      getCredentialStatus: vi.fn(() => Promise.resolve('valid')),
      getConversationUsage: vi.fn(() => Promise.resolve({ total_tokens: 0, turns: [] })),
      getConversationSlug: vi.fn(),
      continueConversation: vi.fn(),
      getSystemPrompt: vi.fn(() => Promise.resolve({ system_prompt: null })),
      getWorkScopeInventory: vi.fn(() => Promise.resolve({ bash: [], tmux: null, browser: null })),
      getLlmLanguageSetting: vi.fn(() => Promise.resolve({ language: 'en' })),
      getVersion: vi.fn(() => Promise.resolve({ version: 'test' })),
      getWakeStatus: vi.fn(() => Promise.resolve({ pending_count: 0, soonest_expires_at: null, contracts: [] })),
      cancelWake: vi.fn(() => Promise.resolve({ success: true })),
    },
  };
});

vi.mock('../cache', () => ({
  cacheDB: {
    getConversation: vi.fn(() => Promise.resolve(null)),
    getConversationBySlug: vi.fn(() => Promise.resolve(null)),
    getMessages: vi.fn(() => Promise.resolve([])),
    getMaxMessageSequenceId: vi.fn(() => Promise.resolve(null)),
    getAllConversations: vi.fn(() => Promise.resolve([])),
    getReplicaMeta: vi.fn(() => Promise.resolve(null)),
    syncConversations: vi.fn(() => Promise.resolve()),
    putConversation: vi.fn(() => Promise.resolve()),
    putMessages: vi.fn(() => Promise.resolve()),
    replaceMessages: vi.fn(() => Promise.resolve()),
    putReplicaMeta: vi.fn(() => Promise.resolve()),
  },
}));

type ConnectionOptions = {
  conversationId?: string;
  dispatch: (action: SSEAction) => void;
  onValidatedInit?: (payload: InitPayload) => void;
  onValidatedSteeringQueued?: (messageId: string) => void;
};

const authoritativeConversations = new Map<string, Conversation>();

function makeConnectionInit(conversation: Conversation): InitPayload {
  return {
    conversation,
    messages: [{ ...historyMessage, conversation_id: conversation.id }],
    steeringMessages: [],
    phase: conversation.state ?? { type: 'idle' },
    contextWindow: { used: 0 },
    transcriptGeneration: conversation.transcript_generation ?? 1,
    streamIncarnation: 'test-stream',
    lastAppliedEventSeq: 0,
    pendingAnchorSequenceId: 0,
    pendingEvents: [],
    pendingTruncated: false,
    transcriptCoverage: 'complete',
  };
}

function useConnectedConnection(options: ConnectionOptions) {
  const { conversationId: connectedConversationId, onValidatedInit } = options;
  const validatedInitRef = useRef(onValidatedInit);
  validatedInitRef.current = onValidatedInit;
  useEffect(() => {
    if (!connectedConversationId) return;
    const conversation = authoritativeConversations.get(connectedConversationId)
      ?? makeConversation({ id: connectedConversationId });
    validatedInitRef.current?.(makeConnectionInit(conversation));
  }, [connectedConversationId]);
  return { state: 'connected' as const, attempt: 0, nextRetryIn: null, retryNow: vi.fn() };
}

const hooksMockState = vi.hoisted(() => ({
  useConnection: vi.fn(),
}));

vi.mock('../hooks', async () => {
  const actual = await vi.importActual<typeof import('../hooks')>('../hooks');
  return {
    ...actual,
    useConnection: hooksMockState.useConnection,
    useIsDesktop: () => viewportFlags.isDesktop,
    useIsWideDesktop: () => viewportFlags.isWideDesktop,
  };
});

vi.mock('../components/ConversationNavStack', () => ({
  ConversationNavStack: ({
    messages,
    hasOlderMessages,
    onLoadOlderMessages,
    loadingOlderMessages,
    transcriptPositioning,
    olderHistoryError,
    onOpenCommissionReview,
  }: {
    messages: Message[];
    hasOlderMessages?: boolean;
    onLoadOlderMessages?: () => void;
    loadingOlderMessages?: boolean;
    transcriptPositioning?: {
      kind: 'idle';
      view?: { conversationId: string; generation: number; transcriptGeneration: number };
    } | {
      kind: 'positioning';
      command: { kind: string; token: number };
      view?: { conversationId: string; generation: number; transcriptGeneration: number };
    };
    olderHistoryError?: string | null;
    onOpenCommissionReview?: (sequenceId: number) => void;
  }) => {
    navStackProps.onOpenCommissionReview = onOpenCommissionReview;
    return (
      <div>
        <div data-testid="message-history">
          {messages.map((message) => {
            const content = message.content as { text?: string } | { type?: string; text?: string }[];
            const rendered = Array.isArray(content)
              ? content.find((block) => block.type === 'text')?.text
              : content?.text;
            return <div key={message.message_id}>{rendered}</div>;
          })}
        </div>
        <div data-testid="history-message-count">{messages.length}</div>
        <div data-testid="history-has-older">{hasOlderMessages ? 'yes' : 'no'}</div>
        {hasOlderMessages && onLoadOlderMessages && (
          <button type="button" onClick={() => onLoadOlderMessages()}>
            Load older messages
          </button>
        )}
        <div data-testid="history-loading">{loadingOlderMessages ? 'loading' : 'idle'}</div>
        <div data-testid="history-scroll-command">
          {transcriptPositioning?.kind === 'positioning'
            ? `${transcriptPositioning.command.kind}:${transcriptPositioning.command.token}`
            : 'none'}
        </div>
        <div data-testid="history-transcript-generation">{transcriptPositioning?.view?.transcriptGeneration ?? 'none'}</div>
        {olderHistoryError && <div role="alert">{olderHistoryError}</div>}
      </div>
    );
  },
}));

vi.mock('../components/TerminalPanel', () => ({
  TerminalPanel: () => <div data-testid="terminal-panel">terminal</div>,
}));

vi.mock('../components/WorkActions', () => ({
  WorkControlBar: () => <div data-testid="work-control-bar">work actions</div>,
}));

vi.mock('../components/FileExplorer/FileTree', () => ({
  FileTree: ({ rootPath }: { rootPath: string }) => <div data-testid="file-tree">{rootPath}</div>,
}));

const slug = 'archived-idle';
const conversationId = 'conv-archived';

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

function makeConversation(overrides: Partial<Conversation> = {}): Conversation {
  return {
    id: conversationId,
    slug,
    model: 'claude-3-5-sonnet',
    cwd: '/repo/project',
    created_at: '2024-01-01T00:00:00Z',
    updated_at: '2024-01-01T00:00:01Z',
    message_count: 1,
    transcript_generation: 1,
    state: { type: 'idle' },
    archived: false,
    browser_session_active: false,
    terminal_uses_tmux: false,
    worktree_path: '/repo/.phoenix/worktrees/conv-archived',
    work_scope_key: 'worktree:/repo/.phoenix/worktrees/conv-archived',
    conv_mode_label: 'Explore',
    ...overrides,
  } as Conversation;
}

const historyMessage: Message = {
  message_id: 'm1',
  sequence_id: 1,
  conversation_id: conversationId,
  message_type: 'user',
  content: { text: 'keep this history visible' },
  created_at: '2024-01-01T00:00:01Z',
};

const catchUpMessage: Message = {
  message_id: 'm2',
  sequence_id: 2,
  conversation_id: conversationId,
  message_type: 'agent',
  content: [{ type: 'text', text: 'incremental catch-up arrived' }],
  created_at: '2024-01-01T00:00:02Z',
};

function LocationProbe() {
  const location = useLocation();
  return <output data-testid="route-location">{location.pathname}{location.search}{location.hash}</output>;
}

function renderPage(
  conversation: Conversation,
  routeSegment: string = conversation.slug,
  initialSearch = '',
  routePrefix: '/c' | '/global' = '/c',
) {
  authoritativeConversations.set(conversation.id, conversation);
  const store = new ConversationStore();
  store.dispatch(conversation.slug, {
    type: 'set_initial_data',
    conversationId: conversation.id,
    conversation,
    messages: [{ ...historyMessage, conversation_id: conversation.id }],
    phase: conversation.state ?? { type: 'idle' },
    contextWindow: { used: 0 },
    transcriptGeneration: conversation.transcript_generation ?? 1,
  });

  vi.mocked(api.getConversation).mockResolvedValue({
    conversation,
    messages: [historyMessage],
    agent_working: false,
    presentation_mode: 'idle',
    context_window_size: 0,
  });
  vi.mocked(api.getConversationBySlug).mockResolvedValue({
    conversation,
    messages: [historyMessage],
    agent_working: false,
    presentation_mode: 'idle',
    context_window_size: 0,
  });
  vi.mocked(api.getConversationRoute).mockResolvedValue({
    id: conversation.id,
    slug: conversation.slug,
  });
  vi.mocked(api.getConversationRouteBySlug).mockResolvedValue({
    id: conversation.id,
    slug: conversation.slug,
  });

  const draftStore = new DraftStore();
  const page = () => (
    <ConversationContext.Provider value={store}>
      <DraftContext.Provider value={draftStore}>
        <ConversationReadinessProvider>
          <MemoryRouter initialEntries={[`${routePrefix}/${routeSegment}${initialSearch}`]}>
            <Routes>
              <Route
                path={`${routePrefix}/:slug`}
                element={<><DesktopLayout><ConversationPage routePrefix={routePrefix} /></DesktopLayout><LocationProbe /></>}
              />
            </Routes>
          </MemoryRouter>
        </ConversationReadinessProvider>
      </DraftContext.Provider>
    </ConversationContext.Provider>
  );
  const view = render(page());

  return { store, ...view, rerenderPage: () => view.rerender(page()) };
}

afterEach(() => {
  viewportFlags.isDesktop = true;
  viewportFlags.isWideDesktop = true;
  authoritativeConversations.clear();
  vi.clearAllMocks();
  vi.restoreAllMocks();
  hooksMockState.useConnection.mockImplementation(useConnectedConnection);
  localStorage.clear();
});

hooksMockState.useConnection.mockImplementation(useConnectedConnection);

describe('ConversationPage message viewer layout', () => {
  it('keeps a direct fullscreen message open out of split-pane layout', async () => {
    const { container } = renderPage(
      makeConversation(),
      slug,
      '?viewer=message&presentation=fullscreen&message=1',
    );

    await screen.findByText('keep this history visible');
    expect(container.querySelector('.app-split-pane')).not.toBeInTheDocument();
  });

  it('uses split-pane layout for a pane message on wide desktop', async () => {
    const { container } = renderPage(
      makeConversation(),
      slug,
      '?viewer=message&presentation=pane&message=1',
    );

    await waitFor(() => {
      expect(container.querySelector('.app-split-pane')).toBeInTheDocument();
    });
  });
});

describe('ConversationPage message delivery reconciliation', () => {
  it('retires local steering ownership before a cross-client cancellation can re-expose it', async () => {
    const messageId = 'queued-then-cancelled';
    localStorage.setItem(`phoenix:queue:${conversationId}`, JSON.stringify([{
      localId: messageId,
      conversationId,
      text: 'must stay cancelled',
      timestamp: 1,
      status: 'steering_queued',
      acceptedAfterEventSeq: 0,
    }]));
    const { store } = renderPage(makeConversation({
      state: { type: 'llm_requesting', attempt: 1 },
    }));
    await screen.findByText('keep this history visible');

    const connection = hooksMockState.useConnection.mock.calls.at(-1)?.[0] as ConnectionOptions;
    act(() => {
      connection.onValidatedSteeringQueued?.(messageId);
      store.dispatch(slug, {
        type: 'sse_steer_message_queued',
        sequenceId: 1,
        message: { message_id: messageId, text: 'must stay cancelled', images: [], files: [] },
      });
      store.dispatch(slug, {
        type: 'sse_steer_message_cancelled',
        sequenceId: 2,
        messageId,
      });
    });

    await waitFor(() => {
      expect(JSON.parse(localStorage.getItem(`phoenix:queue:${conversationId}`) ?? '[]')).toEqual([]);
    });
    expect(screen.queryByText('must stay cancelled')).not.toBeInTheDocument();
    expect(store.getSnapshot(slug).steeringMessages).toEqual([]);
  });

  it('does not overwrite authoritative idle when SSE completes before the chat POST response', async () => {
    const response = deferred<{ queued: boolean; steering: boolean }>();
    const sendMessage = vi.spyOn(api, 'sendMessage').mockReturnValue(response.promise);
    const { store } = renderPage(makeConversation());

    const textbox = await screen.findByRole('textbox');
    fireEvent.change(textbox, { target: { value: 'race this request' } });
    fireEvent.click(screen.getByRole('button', { name: /send/i }));
    await waitFor(() => expect(sendMessage).toHaveBeenCalledTimes(1));

    act(() => {
      store.dispatch(slug, {
        type: 'sse_state_change',
        sequenceId: 1,
        phase: { type: 'llm_requesting', attempt: 1 },
        stateUpdatedAt: Date.now() + 10,
      });
      store.dispatch(slug, {
        type: 'sse_state_change',
        sequenceId: 2,
        phase: { type: 'idle' },
        stateUpdatedAt: Date.now() + 11,
      });
    });
    expect(store.getSnapshot(slug).phase.type).toBe('idle');

    await act(async () => {
      response.resolve({ queued: true, steering: false });
      await response.promise;
    });

    await waitFor(() => expect(store.getSnapshot(slug).phase.type).toBe('idle'));
  });

  it('rolls back an optimistic phase when expansion rejects before a turn starts', async () => {
    vi.spyOn(api, 'sendMessage').mockRejectedValue(new ExpansionError({
      error: 'No matching reference',
      error_type: 'file_not_found',
      reference: '@missing',
    }));
    const { store } = renderPage(makeConversation());

    const textbox = await screen.findByRole('textbox');
    fireEvent.change(textbox, { target: { value: '@missing' } });
    fireEvent.click(screen.getByRole('button', { name: /send/i }));

    await waitFor(() => expect(store.getSnapshot(slug).phase.type).toBe('idle'));
  });

  it('optimistically leaves a resumable error while an accepted retry awaits SSE', async () => {
    const response = deferred<{ queued: boolean; steering: boolean }>();
    vi.spyOn(api, 'sendMessage').mockReturnValue(response.promise);
    const errorState = {
      type: 'error' as const,
      message: 'retryable',
      error_kind: 'server_overloaded' as const,
      error: {
        kind: 'server_overloaded' as const,
        can_auto_retry: true,
        can_user_resume: true,
      },
    };
    const { store } = renderPage(makeConversation({ state: errorState }));
    await waitFor(() => expect(hooksMockState.useConnection).toHaveBeenCalled());
    const options = hooksMockState.useConnection.mock.calls.at(-1)?.[0] as ConnectionOptions;
    act(() => options.dispatch({
      type: 'sse_init',
      payload: makeConnectionInit(makeConversation({ state: errorState })),
    }));

    const textbox = await screen.findByRole('textbox');
    fireEvent.change(textbox, { target: { value: 'retry from error' } });
    const form = textbox.closest('form');
    const sendButton = form?.querySelector<HTMLButtonElement>('button[type="submit"]');
    expect(sendButton).not.toBeNull();
    fireEvent.click(sendButton!);

    await waitFor(() => expect(store.getSnapshot(slug).phase.type).toBe('awaiting_llm'));
    response.resolve({ queued: true, steering: false });
    await response.promise;
    expect(store.getSnapshot(slug).phase.type).toBe('awaiting_llm');
  });

  it('rolls back an optimistic phase after a steering response despite non-phase SSE traffic', async () => {
    const response = deferred<{ queued: boolean; steering: boolean }>();
    vi.spyOn(api, 'sendMessage').mockReturnValue(response.promise);
    const { store } = renderPage(makeConversation());

    const textbox = await screen.findByRole('textbox');
    fireEvent.change(textbox, { target: { value: 'becomes steering' } });
    fireEvent.click(screen.getByRole('button', { name: /send/i }));
    await waitFor(() => expect(store.getSnapshot(slug).phase.type).toBe('awaiting_llm'));

    act(() => {
      store.dispatch(slug, { type: 'sse_sequence_consumed', sequenceId: 1 });
    });
    await act(async () => {
      response.resolve({ queued: true, steering: true });
      await response.promise;
    });

    await waitFor(() => expect(store.getSnapshot(slug).phase.type).toBe('idle'));
  });

  it('rolls back an optimistic phase after a failed POST despite non-phase SSE traffic', async () => {
    const response = deferred<{ queued: boolean; steering: boolean }>();
    vi.spyOn(api, 'sendMessage').mockReturnValue(response.promise);
    const { store } = renderPage(makeConversation());

    const textbox = await screen.findByRole('textbox');
    fireEvent.change(textbox, { target: { value: 'failed request' } });
    fireEvent.click(screen.getByRole('button', { name: /send/i }));
    await waitFor(() => expect(store.getSnapshot(slug).phase.type).toBe('awaiting_llm'));

    act(() => {
      store.dispatch(slug, { type: 'sse_sequence_consumed', sequenceId: 1 });
    });
    await act(async () => {
      response.reject(new Error('network failed'));
      await response.promise.catch(() => undefined);
    });

    await waitFor(() => expect(store.getSnapshot(slug).phase.type).toBe('idle'));
  });

  it('rolls back and reconciles an idempotent direct replay without waiting for SSE', async () => {
    const sendMessage = vi.spyOn(api, 'sendMessage').mockResolvedValue({
      queued: true,
      steering: false,
      already_persisted: true,
    });
    vi.mocked(api.reconcileAcceptedMessages).mockResolvedValue({
      conversation_idle: true,
      entries: [{
        message_id: 'idempotent-replay',
        status: 'persisted',
        message: { ...historyMessage, message_id: 'idempotent-replay', sequence_id: 20 },
      }],
    });
    localStorage.setItem(`phoenix:queue:${conversationId}`, JSON.stringify([{
      localId: 'idempotent-replay',
      conversationId,
      text: 'already persisted',
      timestamp: 1,
      status: 'pending',
    }]));
    const { store } = renderPage(makeConversation());

    await waitFor(() => expect(sendMessage).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(store.getSnapshot(slug).phase.type).toBe('idle'));
    await waitFor(() => expect(api.reconcileAcceptedMessages).toHaveBeenCalled());
    await waitFor(() => {
      expect(JSON.parse(localStorage.getItem(`phoenix:queue:${conversationId}`) ?? '[]')).toEqual([]);
    });
  });

  it('keeps a fresh direct message retryable while its SSE echo is missing', async () => {
    const sendMessage = vi.spyOn(api, 'sendMessage').mockResolvedValue({
      queued: true,
      steering: false,
    });
    renderPage(makeConversation());

    const textbox = await screen.findByRole('textbox');
    fireEvent.change(textbox, { target: { value: 'accepted without echo' } });
    fireEvent.click(screen.getByRole('button', { name: /send/i }));

    await waitFor(() => expect(sendMessage).toHaveBeenCalledTimes(1));
    await waitFor(() => {
      const queue = JSON.parse(
        localStorage.getItem(`phoenix:queue:${conversationId}`) ?? '[]',
      ) as Array<{ status: string }>;
      expect(queue[0]?.status).toBe('pending');
    });
    expect(sendMessage).toHaveBeenCalledTimes(1);
  });

  it('prevents overlapping composer submissions while the first POST is unresolved', async () => {
    const response = deferred<{ queued: boolean; steering: boolean }>();
    const sendMessage = vi.spyOn(api, 'sendMessage').mockReturnValue(response.promise);
    renderPage(makeConversation());

    const textbox = await screen.findByRole('textbox');
    const sendButton = screen.getByRole('button', { name: /send/i });
    fireEvent.change(textbox, { target: { value: 'first submission' } });
    fireEvent.click(sendButton);
    fireEvent.change(textbox, { target: { value: 'overlapping submission' } });
    fireEvent.click(sendButton);

    await waitFor(() => expect(sendMessage).toHaveBeenCalledTimes(1));
    expect(
      JSON.parse(localStorage.getItem(`phoenix:queue:${conversationId}`) ?? '[]'),
    ).toHaveLength(1);
    response.resolve({ queued: true, steering: false });
    await response.promise;
  });

  it('reconciles accepted messages from a non-working error phase', async () => {
    const acceptedId = 'accepted-error';
    localStorage.setItem(`phoenix:queue:${conversationId}`, JSON.stringify([{
      localId: acceptedId,
      conversationId,
      text: 'accepted before error',
      timestamp: 1,
      status: 'accepted',
      acceptedAfterEventSeq: 0,
    }]));
    vi.mocked(api.reconcileAcceptedMessages).mockResolvedValue({
      conversation_idle: true,
      entries: [{
        message_id: acceptedId,
        status: 'persisted',
        message: { ...historyMessage, message_id: acceptedId, sequence_id: 20 },
      }],
    });
    const { store } = renderPage(makeConversation({
      state: { type: 'error', message: 'retryable', error_kind: 'server_error' },
    }));

    await waitFor(() => expect(api.reconcileAcceptedMessages).toHaveBeenCalled());
    await waitFor(() => {
      expect(JSON.parse(localStorage.getItem(`phoenix:queue:${conversationId}`) ?? '[]')).toEqual([]);
    });
    expect(store.getSnapshot(slug).messages.map((message) => message.message_id)).toContain(acceptedId);
  });

  it('exact-ID reconciles an accepted direct message after its SSE echo is missed', async () => {
    const acceptedId = 'accepted-direct';
    localStorage.setItem(`phoenix:queue:${conversationId}`, JSON.stringify([{
      localId: acceptedId,
      conversationId,
      text: 'accepted direct',
      timestamp: 1,
      status: 'accepted',
      acceptedAfterEventSeq: 0,
    }]));
    vi.mocked(api.reconcileAcceptedMessages).mockResolvedValue({
      conversation_idle: true,
      entries: [{
        message_id: acceptedId,
        status: 'persisted',
        message: { ...historyMessage, message_id: acceptedId, sequence_id: 20 },
      }],
    });
    const { store } = renderPage(makeConversation({
      state: { type: 'llm_requesting', attempt: 1 },
    }));

    await screen.findByText('keep this history visible');
    act(() => {
      store.dispatch(slug, {
        type: 'sse_state_change',
        sequenceId: 1,
        phase: { type: 'idle' },
        stateUpdatedAt: Date.now() + 5,
      });
    });

    await waitFor(() => {
      expect(JSON.parse(localStorage.getItem(`phoenix:queue:${conversationId}`) ?? '[]')).toEqual([]);
    });
    expect(store.getSnapshot(slug).messages.map((message) => message.message_id)).toContain(acceptedId);
  });

  it('chunks more than 100 accepted entries within the reconciliation API limit', async () => {
    const acceptedIds = Array.from({ length: 205 }, (_, index) => `accepted-${index}`);
    localStorage.setItem(`phoenix:queue:${conversationId}`, JSON.stringify(acceptedIds.map((localId) => ({
      localId,
      conversationId,
      text: localId,
      timestamp: 1,
      status: 'steering_queued',
      acceptedAfterEventSeq: 0,
    }))));
    vi.mocked(api.reconcileAcceptedMessages).mockImplementation(async (_conversationId, ids) => ({
      conversation_idle: true,
      entries: ids.map((messageId, index) => ({
        message_id: messageId,
        status: 'persisted' as const,
        message: { ...historyMessage, message_id: messageId, sequence_id: index + 20 },
      })),
    }));
    const { store } = renderPage(makeConversation({
      state: { type: 'llm_requesting', attempt: 1 },
    }));

    await screen.findByText('keep this history visible');
    act(() => {
      store.dispatch(slug, {
        type: 'sse_state_change',
        sequenceId: 1,
        phase: { type: 'idle' },
        stateUpdatedAt: Date.now() + 5,
      });
    });

    await waitFor(() => expect(api.reconcileAcceptedMessages).toHaveBeenCalledTimes(3));
    expect(vi.mocked(api.reconcileAcceptedMessages).mock.calls.map((call) => call[1].length)).toEqual([
      100, 100, 5,
    ]);
  });

  it('retries idle reconciliation when connectivity returns after a transient failure', async () => {
    const acceptedId = 'accepted-reconnect';
    localStorage.setItem(`phoenix:queue:${conversationId}`, JSON.stringify([{
      localId: acceptedId,
      conversationId,
      text: 'queued reconnect',
      timestamp: 1,
      status: 'steering_queued',
      acceptedAfterEventSeq: 0,
    }]));
    const firstAttempt = deferred<Awaited<ReturnType<typeof api.reconcileAcceptedMessages>>>();
    vi.mocked(api.reconcileAcceptedMessages)
      .mockImplementationOnce(() => firstAttempt.promise)
      .mockResolvedValueOnce({
        conversation_idle: true,
        entries: [{
          message_id: acceptedId,
          status: 'persisted',
          message: { ...historyMessage, message_id: acceptedId, sequence_id: 20 },
        }],
      });
    hooksMockState.useConnection.mockReturnValue({
      state: 'connected', attempt: 0, nextRetryIn: null, retryNow: vi.fn(),
    });
    const { store, rerenderPage } = renderPage(makeConversation({
      state: { type: 'llm_requesting', attempt: 1 },
    }));

    await screen.findByText('keep this history visible');
    act(() => {
      store.dispatch(slug, {
        type: 'sse_state_change',
        sequenceId: 1,
        phase: { type: 'idle' },
        stateUpdatedAt: Date.now() + 5,
      });
    });
    await waitFor(() => expect(api.reconcileAcceptedMessages).toHaveBeenCalledTimes(1));
    await act(async () => {
      firstAttempt.reject(new Error('offline'));
      await firstAttempt.promise.catch(() => {});
    });

    hooksMockState.useConnection.mockReturnValue({
      state: 'offline', attempt: 1, nextRetryIn: null, retryNow: vi.fn(),
    });
    rerenderPage();
    hooksMockState.useConnection.mockReturnValue({
      state: 'reconnected', attempt: 0, nextRetryIn: null, retryNow: vi.fn(),
    });
    rerenderPage();

    await waitFor(() => expect(api.reconcileAcceptedMessages).toHaveBeenCalledTimes(2));
    await waitFor(() => {
      expect(JSON.parse(localStorage.getItem(`phoenix:queue:${conversationId}`) ?? '[]')).toEqual([]);
    });
  });

  it('compacts accepted steering entries after newer authoritative idle history contains their IDs', async () => {
    const acceptedIds = ['accepted-1', 'accepted-2'];
    localStorage.setItem(`phoenix:queue:${conversationId}`, JSON.stringify(acceptedIds.map((localId) => ({
      localId,
      conversationId,
      text: `queued ${localId}`,
      timestamp: 1,
      status: 'steering_queued',
      acceptedAfterEventSeq: 0,
    }))));
    vi.mocked(api.reconcileAcceptedMessages).mockResolvedValue({
      conversation_idle: true,
      entries: acceptedIds.map((messageId, index) => ({
        message_id: messageId,
        status: 'persisted',
        message: {
          ...historyMessage,
          message_id: messageId,
          sequence_id: index + 20,
        },
      })),
    });
    const sendMessage = vi.spyOn(api, 'sendMessage');
    const { store } = renderPage(makeConversation({
      state: { type: 'llm_requesting', attempt: 1 },
    }));

    await screen.findByText('keep this history visible');
    act(() => {
      store.dispatch(slug, {
        type: 'sse_state_change',
        sequenceId: 1,
        phase: { type: 'idle' },
        stateUpdatedAt: Date.now() + 5,
      });
    });

    await waitFor(() => {
      expect(JSON.parse(localStorage.getItem(`phoenix:queue:${conversationId}`) ?? '[]')).toEqual([]);
    });
    expect(sendMessage).not.toHaveBeenCalled();
    expect(store.getSnapshot(slug).messages.map((message) => message.message_id)).toEqual(
      expect.arrayContaining(acceptedIds),
    );
  });
});

describe('owned conversation navigation', () => {
  it('drops a successful slug resolution after the initiating conversation changes', async () => {
    let resolveSlug!: (slug: string) => void;
    vi.mocked(api.getConversationSlug).mockReturnValue(new Promise((resolve) => { resolveSlug = resolve; }));
    let ownerGeneration = 1;

    const pending = resolveOwnedConversationTarget(
      'successor-id',
      ownerGeneration,
      () => ownerGeneration,
      'Failed to open work conversation',
    );
    ownerGeneration += 1;
    resolveSlug('successor-slug');

    await expect(pending).resolves.toEqual({ kind: 'stale' });
  });

  it('drops a failed slug resolution after the initiating conversation changes', async () => {
    let rejectSlug!: (error: Error) => void;
    vi.mocked(api.getConversationSlug).mockReturnValue(new Promise((_resolve, reject) => { rejectSlug = reject; }));
    let ownerGeneration = 1;

    const pending = resolveOwnedConversationTarget(
      'fork-id',
      ownerGeneration,
      () => ownerGeneration,
      'Created fork conversation.',
    );
    ownerGeneration += 1;
    rejectSlug(new Error('network unavailable'));

    await expect(pending).resolves.toEqual({ kind: 'stale' });
  });

  it('drops a resolution after an owner leaves and returns to the same route identity', async () => {
    let resolveSlug!: (slug: string) => void;
    vi.mocked(api.getConversationSlug).mockReturnValue(new Promise((resolve) => { resolveSlug = resolve; }));
    let ownerGeneration = 1;

    const pending = resolveOwnedConversationTarget(
      'successor-id',
      ownerGeneration,
      () => ownerGeneration,
      'Failed to open work conversation',
    );
    ownerGeneration += 2;
    resolveSlug('successor-slug');

    await expect(pending).resolves.toEqual({ kind: 'stale' });
  });

  it('returns a target only while the initiating conversation still owns navigation', async () => {
    vi.mocked(api.getConversationSlug).mockResolvedValue('successor-slug');

    await expect(resolveOwnedConversationTarget(
      'successor-id',
      1,
      () => 1,
      'Failed to open work conversation',
    )).resolves.toEqual({ kind: 'found', slug: 'successor-slug' });
  });
});

describe('ConversationPage context exhausted handoff', () => {
  it('dispatch_failed keeps the edit on the parent for durable retry', async () => {
    vi.mocked(api.continueConversation).mockResolvedValue({
      status: 'dispatch_failed',
      conversation_id: 'successor-1',
      slug: 'successor-1',
      error: 'Dispatch failed upstream',
    } as never);

    renderPage(makeConversation({
      state: { type: 'context_exhausted', summary: 'Generated summary' },
      conv_mode_label: 'Work',
    }));

    fireEvent.click(await screen.findByRole('button', { name: 'Edit first' }));
    const handoff = await screen.findByTestId('context-exhausted-handoff');
    fireEvent.change(handoff, { target: { value: 'Edited handoff for successor' } });
    fireEvent.click(screen.getByRole('button', { name: 'Continue with edits' }));

    await waitFor(() => expect(api.continueConversation).toHaveBeenCalledWith(
      conversationId,
      expect.objectContaining({ handoff: 'Edited handoff for successor', message_id: expect.any(String) }),
    ));
    expect(await screen.findByText('Dispatch failed upstream')).toBeInTheDocument();
    expect(screen.getByTestId('context-exhausted-handoff')).toHaveValue('Edited handoff for successor');
    expect(localStorage.getItem('seed-draft:successor-1')).toBeNull();
  });

  it.skip('keeps an existing continuation retry failure on the parent', async () => {
    vi.mocked(api.continueConversation).mockResolvedValue({
      status: 'dispatch_failed',
      conversation_id: 'successor-pending',
      error: 'Still unavailable',
    } as never);

    renderPage(makeConversation({
      state: { type: 'context_exhausted', summary: 'Generated summary' },
      conv_mode_label: 'Work',
      continued_in_conv_id: 'successor-pending',
    }));

    const continuation = await screen.findByTestId('continuation-link');
    await act(async () => fireEvent.click(continuation));
    expect(await screen.findByText('Still unavailable')).toBeInTheDocument();
    expect(screen.getByTestId('continuation-link')).toBeInTheDocument();
  });

  it.skip('opens the existing continuation instead of re-seeding the generated summary', async () => {
    vi.mocked(api.continueConversation).mockResolvedValue({
      status: 'already_exists',
      conversation_id: 'successor-2',
      slug: 'successor-2',
    } as never);

    renderPage(makeConversation({
      state: { type: 'context_exhausted', summary: 'Generated summary' },
      conv_mode_label: 'Work',
      continued_in_conv_id: 'successor-2',
    }));

    expect(await screen.findByText('Generated summary')).toBeInTheDocument();
    expect(screen.queryByTestId('context-exhausted-handoff')).not.toBeInTheDocument();
    await act(async () => fireEvent.click(screen.getByTestId('continuation-link')));

    await waitFor(() => expect(api.continueConversation).toHaveBeenCalledWith(
      conversationId,
      expect.objectContaining({ handoff: 'Generated summary', message_id: expect.any(String) }),
    ));
    expect(localStorage.getItem('seed-draft:successor-2')).toBeNull();
  });
});

describe('ConversationPage archived read-only rendering', () => {
  it('keeps archived error conversations free of work actions', async () => {
    renderPage(makeConversation({
      archived: true,
      conv_mode_label: 'Work',
      state: { type: 'error', message: 'Historical failure', error_kind: 'server_error' },
    }));

    expect(await screen.findByText('Historical failure')).toBeInTheDocument();
    expect(screen.queryByTestId('work-control-bar')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /clean up/i })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /abandon/i })).not.toBeInTheDocument();
  });

  it('shows message history but hides composer, work actions, terminal, files, and work scope for archived idle conversations', async () => {
    renderPage(makeConversation({ archived: true }));

    expect(await screen.findByText('keep this history visible')).toBeInTheDocument();
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /send/i })).not.toBeInTheDocument();
    expect(screen.queryByTestId('terminal-panel')).not.toBeInTheDocument();
    expect(screen.queryByTestId('work-control-bar')).not.toBeInTheDocument();
    expect(screen.queryByTestId('file-tree')).not.toBeInTheDocument();
    expect(screen.queryByText(/^Files$/)).not.toBeInTheDocument();
    expect(screen.queryByText(/Work \d+/)).not.toBeInTheDocument();
    expect(screen.getByText('MCP')).toBeInTheDocument();
    expect(screen.getByText('Skills')).toBeInTheDocument();
  });

  it('keeps composer, work actions, terminal, files, and work scope for non-archived idle conversations', async () => {
    renderPage(makeConversation());

    expect(await screen.findByText('keep this history visible')).toBeInTheDocument();
    expect(await screen.findByRole('textbox')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /send/i })).toBeInTheDocument();
    expect(await screen.findByTestId('terminal-panel')).toBeInTheDocument();
    expect(screen.getByTestId('work-control-bar')).toBeInTheDocument();
    expect(screen.getByTestId('file-tree')).toHaveTextContent('/repo/.phoenix/worktrees/conv-archived');
    expect(screen.getByRole('button', { name: /Work/ })).toBeInTheDocument();
  });

  it('moves the mobile terminal out of persistent conversation chrome into a launcher sheet', async () => {
    viewportFlags.isDesktop = false;
    viewportFlags.isWideDesktop = false;
    Object.defineProperty(window, 'matchMedia', {
      configurable: true,
      value: vi.fn().mockImplementation((query: string) => ({
        matches: query === '(max-width: 1024px)',
        media: query,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
      })),
    });

    renderPage(makeConversation());

    expect(await screen.findByText('keep this history visible')).toBeInTheDocument();
    expect(await screen.findByRole('textbox')).toBeInTheDocument();
    const terminal = await screen.findByTestId('terminal-panel');
    expect(document.querySelector('.conversation-column')).not.toContainElement(terminal);
    expect(document.querySelector('.conversation-column')).toContainElement(document.querySelector('#state-bar'));
    expect(document.querySelector('.mobile-terminal-sheet')).toContainElement(terminal);

    expect(document.querySelector('.mobile-terminal-sheet')).not.toHaveClass('mobile-terminal-sheet--open');

    fireEvent.click(document.querySelector('.statebar-chevron')!);
    fireEvent.click(screen.getByRole('button', { name: /Open terminal/ }));
    const close = screen.getByRole('button', { name: 'Close terminal' });
    expect(close).toHaveFocus();
    fireEvent.keyDown(close, { key: 'Tab', shiftKey: true });
    expect(close).toHaveFocus();
    expect(document.querySelector('.conversation-column')).toHaveProperty('inert', true);

    fireEvent.click(close);
    expect(document.querySelector('.conversation-column')).toHaveProperty('inert', false);
    return waitFor(() => {
      expect(screen.getByRole('button', { name: /Open terminal/ })).toHaveFocus();
    });
  });

  it('does not expose a conversation terminal on coordinator routes', async () => {
    viewportFlags.isDesktop = false;
    viewportFlags.isWideDesktop = false;

    const conversation = makeConversation();
    renderPage(conversation, conversation.slug, '', '/global');

    expect(await screen.findByText('keep this history visible')).toBeInTheDocument();
    expect(screen.queryByTestId('terminal-panel')).not.toBeInTheDocument();
    expect(document.querySelector('.mobile-terminal-sheet')).toBeNull();
  });

  it('resolves a UUID route by id before opening its stream', async () => {
    const uuidRoute = '123e4567-e89b-42d3-a456-426614174000';
    const uuidConversation = makeConversation({ id: uuidRoute, slug: 'uuid-archived', archived: true });
    const uuidHistoryMessage = { ...historyMessage, conversation_id: uuidRoute } as Message;
    vi.mocked(cacheDB.getConversation).mockResolvedValue(uuidConversation);
    vi.mocked(cacheDB.getMessages).mockResolvedValue([uuidHistoryMessage]);
    vi.mocked(api.getConversationRoute).mockResolvedValue({
      id: uuidConversation.id,
      slug: uuidConversation.slug,
    });

    renderPage(uuidConversation, uuidRoute);

    expect(await screen.findByText('keep this history visible')).toBeInTheDocument();
    await waitFor(() => expect(api.getConversationRoute).toHaveBeenCalledWith(uuidRoute));
    expect(api.getConversationRouteBySlug).not.toHaveBeenCalledWith(uuidRoute);
    await waitFor(() => {
      const options = hooksMockState.useConnection.mock.calls.at(-1)?.[0] as ConnectionOptions;
      expect(options.conversationId).toBe(uuidConversation.id);
    });
  });

  it('keeps a direct ID route when the authoritative conversation has no slug', async () => {
    const uuidRoute = '123e4567-e89b-42d3-a456-426614174001';
    const uuidConversation = makeConversation({ id: uuidRoute, slug: uuidRoute });
    vi.mocked(api.getConversationRoute).mockResolvedValue({
      id: uuidConversation.id,
      slug: null,
    });

    renderPage(uuidConversation, uuidRoute);

    await waitFor(() => expect(api.getConversationRoute).toHaveBeenCalledWith(uuidRoute));
    expect(api.getConversationRouteBySlug).not.toHaveBeenCalled();
    expect(await screen.findByText('keep this history visible')).toBeInTheDocument();
  });

  it('preserves query and message target when canonicalizing an ID route', async () => {
    const uuidRoute = '123e4567-e89b-42d3-a456-426614174002';
    const conversation = makeConversation({ id: uuidRoute, slug: 'canonical-route' });

    renderPage(conversation, uuidRoute, '?keep=route-state#message-missing-target');

    await waitFor(() => expect(api.getConversationRoute).toHaveBeenCalledWith(uuidRoute));
    expect(await screen.findByText('keep this history visible')).toBeInTheDocument();
    await waitFor(() => expect(screen.getByTestId('route-location')).toHaveTextContent(
      '/c/canonical-route?keep=route-state#message-missing-target',
    ));
  });

  it('keeps commission review actions available for non-terminal narrow layouts', async () => {
    viewportFlags.isWideDesktop = false;
    renderPage(makeConversation());
    await waitFor(() => {
      expect(navStackProps.onOpenCommissionReview).toEqual(expect.any(Function));
    });
  });

  it('hides commission review actions when the conversation cannot open sidepanels', async () => {
    navStackProps.onOpenCommissionReview = vi.fn();
    renderPage(makeConversation({ archived: true }));
    await waitFor(() => {
      expect(navStackProps.onOpenCommissionReview).toBeUndefined();
    });
  });

  it('uses the authoritative route owner when the cached slug owner changed', async () => {
    const staleConversation = makeConversation({ id: 'stale-conv' });
    const authoritativeConversation = makeConversation({ id: 'authoritative-conv' });
    vi.mocked(cacheDB.getConversationBySlug).mockResolvedValue(staleConversation);
    vi.mocked(cacheDB.getMessages).mockResolvedValue([{
      ...historyMessage,
      conversation_id: staleConversation.id,
    }]);
    vi.mocked(api.getConversationRouteBySlug).mockResolvedValue({
      id: authoritativeConversation.id,
      slug: authoritativeConversation.slug,
    });
    authoritativeConversations.set(authoritativeConversation.id, authoritativeConversation);

    render(
      <ConversationContext.Provider value={new ConversationStore()}>
        <DraftContext.Provider value={new DraftStore()}>
          <ConversationReadinessProvider>
            <MemoryRouter initialEntries={[`/c/${slug}`]}>
              <Routes>
                <Route path="/c/:slug" element={<DesktopLayout><ConversationPage /></DesktopLayout>} />
              </Routes>
            </MemoryRouter>
          </ConversationReadinessProvider>
        </DraftContext.Provider>
      </ConversationContext.Provider>,
    );

    expect(await screen.findByText('keep this history visible')).toBeInTheDocument();
    await waitFor(() => {
      const options = hooksMockState.useConnection.mock.calls.at(-1)?.[0] as ConnectionOptions;
      expect(options.conversationId).toBe(authoritativeConversation.id);
    });
    expect(api.getConversationBySlug).not.toHaveBeenCalled();
  });

  it('keeps cached history visible and read-only when route resolution fails', async () => {
    const conversation = makeConversation();
    vi.mocked(cacheDB.getConversationBySlug).mockResolvedValue(conversation);
    vi.mocked(cacheDB.getMessages).mockResolvedValue([historyMessage]);
    const routeFailure = deferred<{ id: string; slug: string | null }>();
    vi.mocked(api.getConversationRouteBySlug).mockReturnValue(routeFailure.promise);

    render(
      <ConversationContext.Provider value={new ConversationStore()}>
        <DraftContext.Provider value={new DraftStore()}>
          <ConversationReadinessProvider>
            <MemoryRouter initialEntries={[`/c/${slug}`]}>
              <Routes>
                <Route path="/c/:slug" element={<DesktopLayout><ConversationPage /></DesktopLayout>} />
              </Routes>
            </MemoryRouter>
          </ConversationReadinessProvider>
        </DraftContext.Provider>
      </ConversationContext.Provider>,
    );
    await waitFor(() => expect(cacheDB.getMessages).toHaveBeenCalledWith(conversation.id));
    await act(async () => {
      routeFailure.reject(new Error('temporary route failure'));
      await routeFailure.promise.catch(() => undefined);
    });

    expect(screen.getByText('keep this history visible')).toBeInTheDocument();
    expect(screen.queryByText('temporary route failure')).not.toBeInTheDocument();
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();

    vi.mocked(api.getConversationRouteBySlug).mockResolvedValue({
      id: conversation.id,
      slug: conversation.slug,
    });
    act(() => window.dispatchEvent(new Event('online')));
    await waitFor(() => {
      const options = hooksMockState.useConnection.mock.calls.at(-1)?.[0] as ConnectionOptions;
      expect(options.conversationId).toBe(conversation.id);
    });
  });

  it('does not send cached reconnect credentials to a different route owner', async () => {
    const staleConversation = makeConversation({ id: 'stale-owner', transcript_generation: 9 });
    const authoritativeConversation = makeConversation({ id: 'authoritative-owner' });
    vi.mocked(cacheDB.getConversationBySlug).mockResolvedValue(staleConversation);
    vi.mocked(cacheDB.getMessages).mockResolvedValue([{ ...historyMessage, conversation_id: staleConversation.id }]);
    vi.mocked(api.getConversationRouteBySlug).mockResolvedValue({
      id: authoritativeConversation.id,
      slug: authoritativeConversation.slug,
    });
    authoritativeConversations.set(authoritativeConversation.id, authoritativeConversation);
    const store = new ConversationStore();
    store.dispatch(slug, {
      type: 'set_initial_data',
      conversationId: staleConversation.id,
      conversation: staleConversation,
      messages: [{ ...historyMessage, conversation_id: staleConversation.id }],
      phase: { type: 'idle' },
      contextWindow: { used: 0 },
      transcriptGeneration: 9,
      eventCursorFloor: 7,
    });

    render(
      <ConversationContext.Provider value={store}>
        <DraftContext.Provider value={new DraftStore()}>
          <ConversationReadinessProvider>
            <MemoryRouter initialEntries={[`/c/${slug}`]}>
              <Routes>
                <Route path="/c/:slug" element={<DesktopLayout><ConversationPage /></DesktopLayout>} />
              </Routes>
            </MemoryRouter>
          </ConversationReadinessProvider>
        </DraftContext.Provider>
      </ConversationContext.Provider>,
    );

    await waitFor(() => {
      const options = hooksMockState.useConnection.mock.calls.at(-1)?.[0] as ConnectionOptions & {
        getLastAppliedEventSeq?: () => number;
        getTranscriptGeneration?: () => number | null;
      };
      expect(options.conversationId).toBe(authoritativeConversation.id);
      expect(options.getLastAppliedEventSeq?.()).toBe(0);
      expect(options.getTranscriptGeneration?.()).toBeNull();
    });
  });

  it('resolves an offline route when connectivity returns', async () => {
    const online = vi.spyOn(window.navigator, 'onLine', 'get').mockReturnValue(false);
    const conversation = makeConversation();
    vi.mocked(cacheDB.getConversationBySlug).mockResolvedValue(conversation);
    vi.mocked(cacheDB.getMessages).mockResolvedValue([historyMessage]);

    renderPage(conversation);
    expect(await screen.findByText('keep this history visible')).toBeInTheDocument();
    expect(api.getConversationRouteBySlug).not.toHaveBeenCalled();

    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();

    online.mockReturnValue(true);
    act(() => window.dispatchEvent(new Event('online')));

    await waitFor(() => expect(api.getConversationRouteBySlug).toHaveBeenCalledWith(slug));
    await waitFor(() => {
      const options = hooksMockState.useConnection.mock.calls.at(-1)?.[0] as ConnectionOptions;
      expect(options.conversationId).toBe(conversation.id);
    });
  });

  it('keeps cached history provisional until authoritative SSE init replaces it', async () => {
    const cachedConversation = makeConversation({ transcript_generation: 7 });
    const authoritativeMessage = {
      ...catchUpMessage,
      content: [{ type: 'text', text: 'authoritative SSE tail' }],
    } as Message;
    vi.mocked(cacheDB.getConversationBySlug).mockResolvedValue(cachedConversation);
    vi.mocked(cacheDB.getMessages).mockResolvedValue([historyMessage]);

    const { store } = renderPage(cachedConversation);

    expect(await screen.findByText('keep this history visible')).toBeInTheDocument();
    await waitFor(() => expect(api.getConversationRouteBySlug).toHaveBeenCalledWith(slug));
    const options = hooksMockState.useConnection.mock.calls.at(-1)?.[0] as ConnectionOptions;
    const payload = {
      ...makeConnectionInit(cachedConversation),
      messages: [authoritativeMessage],
      transcriptGeneration: 8,
      transcriptCoverage: 'tail' as const,
    };
    act(() => {
      options.dispatch({ type: 'sse_init', payload });
      options.onValidatedInit?.(payload);
    });

    expect(await screen.findByText('authoritative SSE tail')).toBeInTheDocument();
    expect(screen.queryByText('keep this history visible')).not.toBeInTheDocument();
    expect(store.getSnapshot(slug).transcriptGeneration).toBe(8);
    expect(screen.getByTestId('history-has-older')).toHaveTextContent('yes');
  });

  it('preserves lazy older-history availability across cursor reconnect', async () => {
    const conversation = makeConversation();
    const { store } = renderPage(conversation);
    await screen.findByText('keep this history visible');
    const options = hooksMockState.useConnection.mock.calls.at(-1)?.[0] as ConnectionOptions;
    const tailPayload = {
      ...makeConnectionInit(conversation),
      transcriptCoverage: 'tail' as const,
    };
    act(() => {
      options.dispatch({ type: 'sse_init', payload: tailPayload });
      options.onValidatedInit?.(tailPayload);
    });
    expect(screen.getByTestId('history-has-older')).toHaveTextContent('yes');

    const preservePayload = {
      ...makeConnectionInit(conversation),
      messages: [],
      transcriptCoverage: 'preserve' as const,
    };
    act(() => {
      options.dispatch({ type: 'sse_init', payload: preservePayload });
      options.onValidatedInit?.(preservePayload);
    });

    expect(store.getSnapshot(slug).transcriptCoverage).toBe('tail');
    expect(screen.getByTestId('history-has-older')).toHaveTextContent('yes');
  });

  it('loads older history lazily from REST after SSE reports tail coverage', async () => {
    const newest = { ...catchUpMessage, sequence_id: 2 } as Message;
    vi.mocked(api.getConversation).mockResolvedValue({
      conversation: makeConversation(),
      messages: [historyMessage, newest],
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });
    const { store } = renderPage(makeConversation());
    await screen.findByText('keep this history visible');
    const options = hooksMockState.useConnection.mock.calls.at(-1)?.[0] as ConnectionOptions;
    const payload = {
      ...makeConnectionInit(makeConversation()),
      messages: [newest],
      transcriptCoverage: 'tail' as const,
    };
    act(() => {
      options.dispatch({ type: 'sse_init', payload });
      options.onValidatedInit?.(payload);
    });

    fireEvent.click(await screen.findByRole('button', { name: 'Load older messages' }));

    await waitFor(() => expect(api.getConversation).toHaveBeenCalledWith(conversationId));
    expect(await screen.findByText('keep this history visible')).toBeInTheDocument();
    expect(store.getSnapshot(slug).transcriptCoverage).toBe('complete');
  });

  it('provides only the event cursor and transcript generation for reconnect', async () => {
    renderPage(makeConversation({ transcript_generation: 4 }));
    await waitFor(() => expect(hooksMockState.useConnection).toHaveBeenCalled());
    const options = hooksMockState.useConnection.mock.calls.at(-1)?.[0] as ConnectionOptions & {
      getLastAppliedEventSeq?: () => number;
      getTranscriptGeneration?: () => number | null;
    };

    expect(options.getLastAppliedEventSeq).toEqual(expect.any(Function));
    expect(options.getTranscriptGeneration?.()).toBe(4);
    expect(options).not.toHaveProperty('getInitialRequestMode');
  });

});
