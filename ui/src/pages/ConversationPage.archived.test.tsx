import { describe, it, expect, vi, afterEach } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { ConversationPage } from './ConversationPage';
import { DesktopLayout } from '../components/DesktopLayout';
import { ConversationContext } from '../conversation/ConversationContext';
import { DraftContext } from '../conversation/DraftContext';
import { ConversationStore } from '../conversation';
import { DraftStore } from '../conversation/DraftStore';
import { api, ExpansionError, MessageSliceAlignmentError, type Conversation, type Message } from '../api';
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
      getConversationMeta: vi.fn(),
      getConversationMetaBySlug: vi.fn(),
      getConversationMessagesAfter: vi.fn(),
      getConversationMessagesLatest: vi.fn(),
      reconcileAcceptedMessages: vi.fn(),
      listConversations: vi.fn(() => Promise.resolve([])),
      listArchivedConversations: vi.fn(() => Promise.resolve([])),
      getModels: vi.fn(() => Promise.resolve([])),
      getPrStatus: vi.fn(() => Promise.resolve({ found: false })),
      getCredentialStatus: vi.fn(() => Promise.resolve('valid')),
      getConversationUsage: vi.fn(() => Promise.resolve({ total_tokens: 0, turns: [] })),
      getSystemPrompt: vi.fn(() => Promise.resolve({ system_prompt: null })),
      getWorkScopeInventory: vi.fn(() => Promise.resolve({ bash: [], tmux: null, browser: null })),
      getLlmLanguageSetting: vi.fn(() => Promise.resolve({ language: 'en' })),
      getVersion: vi.fn(() => Promise.resolve({ version: 'test' })),
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

const hooksMockState = vi.hoisted(() => ({
  useConnection: vi.fn(() => ({ state: 'connected', attempt: 0, nextRetryIn: null, retryNow: vi.fn() })),
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

function renderPage(conversation: Conversation, routeSegment: string = conversation.slug, initialSearch = '') {
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
  vi.mocked(api.getConversationMeta).mockResolvedValue({
    conversation,
    agent_working: false,
    presentation_mode: 'idle',
    context_window_size: 0,
  });
  vi.mocked(api.getConversationMetaBySlug).mockResolvedValue({
    conversation,
    agent_working: false,
    presentation_mode: 'idle',
    context_window_size: 0,
  });

  vi.mocked(api.getConversationMetaBySlug).mockResolvedValue({
    conversation,
    agent_working: false,
    presentation_mode: 'idle',
    context_window_size: 0,
  });

  const draftStore = new DraftStore();
  const page = () => (
    <ConversationContext.Provider value={store}>
      <DraftContext.Provider value={draftStore}>
        <ConversationReadinessProvider>
          <MemoryRouter initialEntries={[`/c/${routeSegment}${initialSearch}`]}>
            <Routes>
              <Route path="/c/:slug" element={<DesktopLayout><ConversationPage /></DesktopLayout>} />
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
  vi.clearAllMocks();
  vi.restoreAllMocks();
  vi.mocked(api.getConversationMessagesLatest).mockReset();
  localStorage.clear();
});

describe('ConversationPage message delivery reconciliation', () => {
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
    };
    const { store } = renderPage(makeConversation({ state: errorState }));

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

  it('does not repost a successful direct message while its SSE echo is missing', async () => {
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
      expect(queue[0]?.status).toBe('accepted');
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
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
    vi.mocked(api.reconcileAcceptedMessages)
      .mockRejectedValueOnce(new Error('offline'))
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

describe('ConversationPage archived read-only rendering', () => {
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

  it('keeps the conversation terminal inside mobile conversation chrome', async () => {
    viewportFlags.isDesktop = false;
    viewportFlags.isWideDesktop = false;

    renderPage(makeConversation());

    expect(await screen.findByText('keep this history visible')).toBeInTheDocument();
    expect(await screen.findByRole('textbox')).toBeInTheDocument();
    expect(await screen.findByTestId('terminal-panel')).toBeInTheDocument();
    expect(document.querySelector('.conversation-column')).toContainElement(document.querySelector('[data-testid="terminal-panel"]'));
    expect(document.querySelector('.conversation-column')).toContainElement(document.querySelector('#state-bar'));
  });

  it('cold-loads a UUID route via id metadata and id full-history paths', async () => {
    const uuidRoute = '123e4567-e89b-42d3-a456-426614174000';
    const uuidConversation = makeConversation({ id: uuidRoute, slug: 'uuid-archived', archived: true });
    const uuidHistoryMessage = { ...historyMessage, conversation_id: uuidRoute } as Message;
    const uuidCatchUpMessage = { ...catchUpMessage, conversation_id: uuidRoute } as Message;
    vi.mocked(cacheDB.getConversation).mockResolvedValue(uuidConversation);
    vi.mocked(cacheDB.getMessages).mockResolvedValue([uuidHistoryMessage]);
    vi.mocked(api.getConversationMeta).mockResolvedValue({
      conversation: uuidConversation,
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });
    vi.mocked(api.getConversationMessagesLatest).mockResolvedValue({
      messages: [uuidCatchUpMessage],
      tombstones: [],
      transcript_generation: 1,
      server_message_tail: 2,
      has_older_messages: true,
    });

    renderPage(uuidConversation, uuidRoute);

    expect(await screen.findByText('incremental catch-up arrived')).toBeInTheDocument();
    await waitFor(() => {
      expect(cacheDB.getConversation).toHaveBeenCalledWith(uuidRoute);
    });
    expect(cacheDB.getConversationBySlug).not.toHaveBeenCalledWith(uuidRoute);
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

  it('uses authoritative metadata when the cached slug owner changed', async () => {
    const staleConversation = makeConversation({ id: 'stale-conv' });
    const authoritativeConversation = makeConversation({ id: 'authoritative-conv' });
    vi.mocked(cacheDB.getConversationBySlug).mockResolvedValue(staleConversation);
    vi.mocked(cacheDB.getMessages).mockResolvedValue([{ ...historyMessage, conversation_id: staleConversation.id }]);
    vi.mocked(cacheDB.getMaxMessageSequenceId).mockResolvedValue(1);
    vi.mocked(api.getConversationMessagesAfter).mockResolvedValue({
      messages: [],
      tombstones: [],
      transcript_generation: 1,
      server_message_tail: 1,
      has_older_messages: false,
    });
    vi.mocked(api.getConversationMessagesLatest).mockResolvedValue({
      messages: [],
      tombstones: [],
      transcript_generation: 1,
      server_message_tail: 1,
      has_older_messages: false,
    });
    vi.mocked(api.getConversationMetaBySlug).mockResolvedValue({
      conversation: authoritativeConversation,
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });
    vi.mocked(api.getConversationBySlug).mockResolvedValue({
      conversation: authoritativeConversation,
      messages: [],
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });

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

    await waitFor(() => {
      expect(api.getConversationMetaBySlug).toHaveBeenCalledWith(slug);
    });
    expect(api.getConversationBySlug).not.toHaveBeenCalled();
  });

  it('keeps cached history conservatively tail-covered when the server reports earlier messages', async () => {
    const cachedConversation = makeConversation();
    vi.mocked(cacheDB.getConversationBySlug).mockResolvedValue(cachedConversation);
    vi.mocked(cacheDB.getMessages).mockResolvedValue([historyMessage, catchUpMessage]);
    vi.mocked(cacheDB.getReplicaMeta).mockResolvedValue({
      conversationId,
      latestMessageSequenceId: 2,
      latestEventSequenceId: null,
      transcriptGeneration: 1,
      lastHydratedAt: '2024-01-01T00:00:02Z',
    });
    vi.mocked(api.getConversationMessagesAfter).mockResolvedValue({
      messages: [],
      tombstones: [],
      transcript_generation: 1,
      server_message_tail: 2,
      has_older_messages: false,
    });
    vi.mocked(api.getConversationMessagesLatest).mockResolvedValue({
      messages: [catchUpMessage],
      tombstones: [],
      transcript_generation: 1,
      server_message_tail: 2,
      has_older_messages: true,
    });
    vi.mocked(api.getConversationMetaBySlug).mockResolvedValue({
      conversation: cachedConversation,
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });

    renderPage(cachedConversation);

    expect(await screen.findByText('keep this history visible')).toBeInTheDocument();
    await waitFor(() => expect(api.getConversationMessagesLatest).toHaveBeenCalled());
    expect(screen.getByRole('button', { name: 'Load older messages' })).toBeInTheDocument();
  });

  it('refreshes stale cached rows with authoritative latest when replica meta generation is missing or stale', async () => {
    const cachedConversation = makeConversation({ transcript_generation: 7 });
    const staleCachedTail = {
      ...catchUpMessage,
      content: [{ type: 'text', text: 'stale cached tail' }],
    } as Message;
    const authoritativeTail = {
      ...catchUpMessage,
      content: [{ type: 'text', text: 'authoritative tail' }],
    } as Message;

    vi.mocked(cacheDB.getConversationBySlug).mockResolvedValue(cachedConversation);
    vi.mocked(cacheDB.getMessages).mockResolvedValue([historyMessage, staleCachedTail]);
    vi.mocked(cacheDB.getReplicaMeta).mockResolvedValue({
      conversationId,
      latestMessageSequenceId: 2,
      latestEventSequenceId: null,
      transcriptGeneration: null,
      lastHydratedAt: '2024-01-01T00:00:03Z',
    });
    vi.mocked(api.getConversationMetaBySlug).mockResolvedValue({
      conversation: cachedConversation,
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });
    vi.mocked(api.getConversationMessagesLatest).mockResolvedValue({
      messages: [authoritativeTail],
      tombstones: [],
      transcript_generation: 7,
      server_message_tail: 2,
      has_older_messages: true,
    });

    renderPage(cachedConversation);

    expect(await screen.findByText('authoritative tail')).toBeInTheDocument();
    expect(screen.queryByText('stale cached tail')).not.toBeInTheDocument();
    expect(api.getConversationMessagesAfter).not.toHaveBeenCalled();
    expect(api.getConversationMessagesLatest).toHaveBeenCalledWith(conversationId, 50);
    expect(cacheDB.replaceMessages).toHaveBeenLastCalledWith(conversationId, [authoritativeTail]);
    await waitFor(() => {
      expect(cacheDB.putReplicaMeta).toHaveBeenCalledWith(
        expect.objectContaining({
          conversationId,
          latestMessageSequenceId: 2,
          transcriptGeneration: 7,
        }),
      );
    });
  });

  it('keeps the warm incremental path when cached replica meta generation matches metadata', async () => {
    const cachedConversation = makeConversation({ transcript_generation: 7 });
    vi.mocked(cacheDB.getConversationBySlug).mockResolvedValue(cachedConversation);
    vi.mocked(cacheDB.getMessages).mockResolvedValue([historyMessage]);
    vi.mocked(cacheDB.getReplicaMeta).mockResolvedValue({
      conversationId,
      latestMessageSequenceId: 1,
      latestEventSequenceId: null,
      transcriptGeneration: 7,
      lastHydratedAt: '2024-01-01T00:00:02Z',
    });
    vi.mocked(api.getConversationMetaBySlug).mockResolvedValue({
      conversation: cachedConversation,
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });
    vi.mocked(api.getConversationMessagesAfter)
      .mockResolvedValueOnce({
        messages: [catchUpMessage],
        tombstones: [],
        transcript_generation: 7,
        server_message_tail: 2,
        has_older_messages: false,
      })
      .mockResolvedValueOnce({
        messages: [],
        tombstones: [],
        transcript_generation: 7,
        server_message_tail: 2,
        has_older_messages: false,
      });
    vi.mocked(api.getConversationMessagesLatest).mockResolvedValue({
      messages: [catchUpMessage],
      tombstones: [],
      transcript_generation: 7,
      server_message_tail: 2,
      has_older_messages: false,
    });

    renderPage(cachedConversation);

    expect(await screen.findByText('incremental catch-up arrived')).toBeInTheDocument();
    expect(api.getConversationMessagesAfter).toHaveBeenCalledWith(conversationId, 1, 200);
    expect(api.getConversationMessagesLatest).toHaveBeenCalledWith(conversationId, 50);
  });

  it('retries cold latest-window load after metadata refresh when the transcript generation advances once', async () => {
    const firstMetadata = makeConversation({ transcript_generation: 7 });
    const refreshedMetadata = makeConversation({ transcript_generation: 8, updated_at: '2024-01-01T00:00:09Z' });
    const authoritativeTail = {
      ...catchUpMessage,
      content: [{ type: 'text', text: 'refreshed latest tail' }],
    } as Message;

    vi.mocked(cacheDB.getConversationBySlug).mockResolvedValue(null);
    vi.mocked(cacheDB.getConversation).mockResolvedValue(null);
    vi.mocked(api.getConversationMetaBySlug)
      .mockResolvedValueOnce({
        conversation: firstMetadata,
        agent_working: false,
        presentation_mode: 'idle',
        context_window_size: 0,
      })
      .mockResolvedValueOnce({
        conversation: refreshedMetadata,
        agent_working: false,
        presentation_mode: 'idle',
        context_window_size: 0,
      });
    vi.mocked(api.getConversationMessagesLatest)
      .mockResolvedValueOnce({
        messages: [authoritativeTail],
        tombstones: [],
        transcript_generation: 8,
        server_message_tail: 2,
        has_older_messages: true,
      })
      .mockResolvedValueOnce({
        messages: [authoritativeTail],
        tombstones: [],
        transcript_generation: 8,
        server_message_tail: 2,
        has_older_messages: true,
      });

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

    expect(await screen.findByText('refreshed latest tail')).toBeInTheDocument();
    expect(screen.queryByText('Conversation transcript kept changing while loading')).not.toBeInTheDocument();
    expect(api.getConversationMetaBySlug).toHaveBeenCalledTimes(3);
    expect(api.getConversationMessagesLatest).toHaveBeenCalledTimes(2);
  });

  it('surfaces a changing-transcript error after three consecutive cold-load mismatches', async () => {
    const metadata7 = makeConversation({ transcript_generation: 7 });
    const metadata8 = makeConversation({ transcript_generation: 8, updated_at: '2024-01-01T00:00:08Z' });
    const metadata9 = makeConversation({ transcript_generation: 9, updated_at: '2024-01-01T00:00:09Z' });
    const latestTail = { ...catchUpMessage } as Message;

    vi.mocked(cacheDB.getConversationBySlug).mockResolvedValue(null);
    vi.mocked(cacheDB.getConversation).mockResolvedValue(null);
    vi.mocked(api.getConversationMetaBySlug)
      .mockResolvedValueOnce({ conversation: metadata7, agent_working: false, presentation_mode: 'idle', context_window_size: 0 })
      .mockResolvedValueOnce({ conversation: metadata8, agent_working: false, presentation_mode: 'idle', context_window_size: 0 })
      .mockResolvedValueOnce({ conversation: metadata9, agent_working: false, presentation_mode: 'idle', context_window_size: 0 });
    vi.mocked(api.getConversationMessagesLatest)
      .mockResolvedValueOnce({ messages: [latestTail], tombstones: [], transcript_generation: 8, server_message_tail: 2, has_older_messages: true })
      .mockResolvedValueOnce({ messages: [latestTail], tombstones: [], transcript_generation: 9, server_message_tail: 2, has_older_messages: true })
      .mockResolvedValueOnce({ messages: [latestTail], tombstones: [], transcript_generation: 10, server_message_tail: 2, has_older_messages: true });

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

    expect(await screen.findByText('Conversation transcript kept changing while loading')).toBeInTheDocument();
    expect(api.getConversationMetaBySlug).toHaveBeenCalledTimes(3);
    expect(api.getConversationMessagesLatest).toHaveBeenCalledTimes(3);
  });

  it('fills a tail that grows during latest-window refresh before advancing replica coverage', async () => {
    vi.mocked(api.getConversationMetaBySlug).mockReset();
    vi.mocked(api.getConversationMessagesAfter).mockReset();
    vi.mocked(api.getConversationMessagesLatest).mockReset();
    vi.mocked(cacheDB.getConversationBySlug).mockReset();
    vi.mocked(cacheDB.getMessages).mockReset();
    vi.mocked(cacheDB.getReplicaMeta).mockReset();
    const cachedConversation = makeConversation({ transcript_generation: 7 });
    const message3 = { ...catchUpMessage, message_id: 'm3', sequence_id: 3, content: [{ type: 'text', text: 'middle message' }] } as Message;
    const message4 = { ...catchUpMessage, message_id: 'm4', sequence_id: 4, content: [{ type: 'text', text: 'new tail message' }] } as Message;
    vi.mocked(cacheDB.getConversationBySlug).mockResolvedValue(cachedConversation);
    vi.mocked(cacheDB.getMessages).mockResolvedValue([historyMessage]);
    vi.mocked(cacheDB.getReplicaMeta).mockResolvedValue({
      conversationId,
      latestMessageSequenceId: 1,
      latestEventSequenceId: null,
      transcriptGeneration: 7,
      lastHydratedAt: '2024-01-01T00:00:02Z',
    });
    vi.mocked(cacheDB.getMaxMessageSequenceId).mockResolvedValue(1);
    vi.mocked(api.getConversationMessagesAfter).mockImplementation(async (_id, afterSequence) => (
      afterSequence < 2
        ? {
            messages: [catchUpMessage],
            tombstones: [],
            transcript_generation: 7,
            server_message_tail: 2,
            has_older_messages: false,
          }
        : {
            messages: [message3, message4],
            tombstones: [],
            transcript_generation: 7,
            server_message_tail: 4,
            has_older_messages: false,
          }
    ));
    vi.mocked(api.getConversationMessagesLatest).mockResolvedValue({
      messages: [message4],
      tombstones: [],
      transcript_generation: 7,
      server_message_tail: 4,
      has_older_messages: false,
    });
    vi.mocked(api.getConversationMetaBySlug).mockResolvedValue({
      conversation: cachedConversation,
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });

    renderPage(cachedConversation);

    expect(await screen.findByText('middle message')).toBeInTheDocument();
    expect(await screen.findByText('new tail message')).toBeInTheDocument();
    expect(api.getConversationMessagesAfter).toHaveBeenCalledWith(conversationId, 1, 200);
    expect(api.getConversationMessagesAfter).toHaveBeenCalledWith(conversationId, 2, 200);
    await waitFor(() => {
      expect(cacheDB.putReplicaMeta).toHaveBeenCalledWith(
        expect.objectContaining({ latestMessageSequenceId: 4 }),
      );
    });
  });

  it('records latest transcript generation from warm-cache catch-up responses', async () => {
    const cachedConversation = makeConversation({ transcript_generation: 7 });
    const message3 = {
      ...catchUpMessage,
      message_id: 'm3',
      sequence_id: 3,
      content: [{ type: 'text', text: 'generation eight tail' }],
    } as Message;

    vi.mocked(cacheDB.getConversationBySlug).mockResolvedValue(cachedConversation);
    vi.mocked(cacheDB.getMessages).mockResolvedValue([historyMessage]);
    vi.mocked(cacheDB.getReplicaMeta).mockResolvedValue({
      conversationId,
      latestMessageSequenceId: 1,
      latestEventSequenceId: null,
      transcriptGeneration: 7,
      lastHydratedAt: '2024-01-01T00:00:02Z',
    });
    vi.mocked(cacheDB.getMaxMessageSequenceId).mockResolvedValue(1);
    vi.mocked(api.getConversationMetaBySlug).mockResolvedValue({
      conversation: cachedConversation,
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });
    vi.mocked(api.getConversationMessagesAfter).mockResolvedValue({
      messages: [message3],
      tombstones: [],
      transcript_generation: 8,
      server_message_tail: 3,
      has_older_messages: true,
    });
    vi.mocked(api.getConversationMessagesLatest).mockResolvedValue({
      messages: [message3],
      tombstones: [],
      transcript_generation: 8,
      server_message_tail: 3,
      has_older_messages: true,
    });

    renderPage(cachedConversation);

    expect(await screen.findByText('generation eight tail')).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.getByTestId('history-transcript-generation')).toHaveTextContent('8');
    });
    await waitFor(() => {
      expect(cacheDB.putReplicaMeta).toHaveBeenCalledWith(
        expect.objectContaining({ transcriptGeneration: 8, latestMessageSequenceId: 3 }),
      );
    });
  });

  it('metadata-only archive confirmation arriving during pending latest refresh keeps tail coverage until refresh resolves', async () => {
    const cachedConversation = makeConversation({ transcript_generation: 7 });
    let resolveLatest: undefined | ((value: {
      messages: Message[];
      tombstones: [];
      transcript_generation: number;
      server_message_tail: number;
      has_older_messages: boolean;
    }) => void);
    const latestWindow = new Promise<{
      messages: Message[];
      tombstones: [];
      transcript_generation: number;
      server_message_tail: number;
      has_older_messages: boolean;
    }>((resolve) => {
      resolveLatest = resolve;
    });

    vi.mocked(cacheDB.getConversationBySlug).mockResolvedValue(cachedConversation);
    vi.mocked(cacheDB.getMessages).mockResolvedValue([historyMessage]);
    vi.mocked(cacheDB.getReplicaMeta).mockResolvedValue({
      conversationId,
      latestMessageSequenceId: 1,
      latestEventSequenceId: null,
      transcriptGeneration: 7,
      lastHydratedAt: '2024-01-01T00:00:02Z',
    });
    vi.mocked(cacheDB.getMaxMessageSequenceId).mockResolvedValue(1);
    vi.mocked(api.getConversationMetaBySlug).mockResolvedValue({
      conversation: cachedConversation,
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });
    vi.mocked(api.getConversationMessagesAfter).mockResolvedValue({
      messages: [],
      tombstones: [],
      transcript_generation: 7,
      server_message_tail: 2,
      has_older_messages: true,
    });
    vi.mocked(api.getConversationMessagesLatest).mockReturnValue(latestWindow);

    const { store } = renderPage(cachedConversation);

    expect(await screen.findByText('keep this history visible')).toBeInTheDocument();

    store.dispatch(cachedConversation.slug, {
      type: 'sse_conversation_update',
      sequenceId: 1,
      updates: { archived: true },
    });

    expect(screen.getByTestId('history-has-older')).toHaveTextContent('no');
    expect(screen.getByTestId('history-transcript-generation')).toHaveTextContent('7');

    resolveLatest?.({
      messages: [historyMessage],
      tombstones: [],
      transcript_generation: 7,
      server_message_tail: 1,
      has_older_messages: false,
    });

    await waitFor(() => {
      expect(screen.getByTestId('history-loading')).toHaveTextContent('idle');
    });
  });

  it('metadata-only archive confirmation arriving during failed latest refresh keeps tail coverage', async () => {
    const cachedConversation = makeConversation({ transcript_generation: 7 });

    vi.mocked(cacheDB.getConversationBySlug).mockResolvedValue(cachedConversation);
    vi.mocked(cacheDB.getMessages).mockResolvedValue([historyMessage]);
    vi.mocked(cacheDB.getReplicaMeta).mockResolvedValue({
      conversationId,
      latestMessageSequenceId: 1,
      latestEventSequenceId: null,
      transcriptGeneration: 7,
      lastHydratedAt: '2024-01-01T00:00:02Z',
    });
    vi.mocked(cacheDB.getMaxMessageSequenceId).mockResolvedValue(1);
    vi.mocked(api.getConversationMetaBySlug).mockResolvedValue({
      conversation: cachedConversation,
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });
    vi.mocked(api.getConversationMessagesAfter).mockResolvedValue({
      messages: [],
      tombstones: [],
      transcript_generation: 7,
      server_message_tail: 2,
      has_older_messages: true,
    });
    vi.mocked(api.getConversationMessagesLatest).mockRejectedValue(new Error('latest failed'));

    const { store } = renderPage(cachedConversation);

    expect(await screen.findByText('keep this history visible')).toBeInTheDocument();

    store.dispatch(cachedConversation.slug, {
      type: 'sse_conversation_update',
      sequenceId: 1,
      updates: { archived: true },
    });

    expect(screen.getByTestId('history-has-older')).toHaveTextContent('no');
    expect(screen.getByTestId('history-transcript-generation')).toHaveTextContent('7');
    expect(await screen.findByTestId('history-loading')).toHaveTextContent('idle');
  });

  it('uses the id metadata path for UUID-route archive confirmation', async () => {
    const uuidRoute = '123e4567-e89b-42d3-a456-426614174000';
    const uuidConversation = makeConversation({ id: uuidRoute, slug: 'uuid-expand', archived: false });
    vi.mocked(cacheDB.getConversation).mockResolvedValue(uuidConversation);
    vi.mocked(cacheDB.getMessages).mockResolvedValue([catchUpMessage]);
    vi.mocked(api.getConversationMeta).mockResolvedValue({
      conversation: uuidConversation,
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });
    vi.mocked(api.getConversationMessagesLatest).mockResolvedValue({
      messages: [catchUpMessage],
      tombstones: [],
      transcript_generation: 1,
      server_message_tail: 2,
      has_older_messages: true,
    });
    vi.mocked(api.getConversation).mockResolvedValue({
      conversation: uuidConversation,
      messages: [historyMessage, catchUpMessage],
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });

    renderPage(uuidConversation, uuidRoute);

    expect(await screen.findByText('incremental catch-up arrived')).toBeInTheDocument();
    await waitFor(() => {
      expect(api.getConversationMeta).toHaveBeenCalledWith(uuidRoute);
    });
    expect(api.getConversation).not.toHaveBeenCalled();
    expect(api.getConversationBySlug).not.toHaveBeenCalledWith(uuidRoute);
  });

  it('requests a generation-bound SSE suffix for an initialized message tail', async () => {
    const store = new ConversationStore();
    const conversation = makeConversation({ transcript_generation: 1 });
    const latestWindowMessage = {
      ...catchUpMessage,
      message_id: 'm-latest-only',
      sequence_id: 2,
      content: [{ type: 'text', text: 'latest-window message' }],
    } as Message;
    store.dispatch(slug, {
      type: 'set_initial_data',
      conversationId,
      conversation,
      messages: [historyMessage, latestWindowMessage],
      phase: { type: 'idle' },
      contextWindow: { used: 0 },
      transcriptGeneration: 1,
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

    await waitFor(() => expect(hooksMockState.useConnection).toHaveBeenCalled());
    const options = (hooksMockState.useConnection.mock.calls as unknown as Array<[unknown]>).at(-1)![0] as {
      getInitialRequestMode?: () => { kind: string; afterMessageFloor?: number; transcriptGeneration?: number };
    };
    expect(options.getInitialRequestMode?.()).toEqual({
      kind: 'messages_after_floor',
      afterMessageFloor: 2,
      transcriptGeneration: 1,
    });
  });

  it('falls back to a full archived conversation fetch when cold latest-window load hits MessageSliceAlignmentError', async () => {
    const archivedConversation = makeConversation({ archived: true, transcript_generation: 3 });
    const fallbackMessage = {
      ...catchUpMessage,
      message_id: 'm-fallback',
      sequence_id: 2,
      content: [{ type: 'text', text: 'full fetch fallback message' }],
    } as Message;

    vi.mocked(cacheDB.getConversationBySlug).mockResolvedValue(null);
    vi.mocked(cacheDB.getConversation).mockResolvedValue(null);
    vi.mocked(api.getConversationMetaBySlug).mockResolvedValue({
      conversation: archivedConversation,
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });
    vi.mocked(api.getConversationMessagesLatest).mockRejectedValue(
      new MessageSliceAlignmentError('Aligned message slice exceeds the server response ceiling of 100 messages'),
    );
    vi.mocked(api.getConversation).mockResolvedValue({
      conversation: archivedConversation,
      messages: [historyMessage, fallbackMessage],
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });

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

    expect(await screen.findByText('full fetch fallback message')).toBeInTheDocument();
    expect(screen.getByTestId('history-message-count')).toHaveTextContent('2');
    expect(screen.getByTestId('history-has-older')).toHaveTextContent('no');
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    await waitFor(() => expect(api.getConversation).toHaveBeenCalledWith(archivedConversation.id));
    expect(api.getConversationBySlug).not.toHaveBeenCalled();
  });

  it('does not fall back to a full fetch for unrelated latest-window cold-load failures', async () => {
    const archivedConversation = makeConversation({ archived: true, transcript_generation: 3 });

    vi.mocked(cacheDB.getConversationBySlug).mockResolvedValue(null);
    vi.mocked(cacheDB.getConversation).mockResolvedValue(null);
    vi.mocked(api.getConversationMetaBySlug).mockResolvedValue({
      conversation: archivedConversation,
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });
    vi.mocked(api.getConversationMessagesLatest).mockRejectedValue(new Error('database offline'));
    vi.mocked(api.getConversationBySlug).mockClear();

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

    expect(await screen.findByText('database offline')).toBeInTheDocument();
    expect(api.getConversationBySlug).not.toHaveBeenCalled();
  });

  it('rejects full-history responses when a slug resolves to a different conversation', async () => {
    const cachedConversation = makeConversation({ transcript_generation: 1 });
    const replacementConversation = makeConversation({ id: 'replacement-conversation', transcript_generation: 1 });
    const replacementMessage = {
      ...catchUpMessage,
      message_id: 'replacement-message',
      conversation_id: replacementConversation.id,
      content: [{ type: 'text', text: 'wrong conversation history' }],
    } as Message;

    const partialHistoryMessage = { ...historyMessage, sequence_id: 2 } as Message;
    vi.mocked(cacheDB.getConversationBySlug).mockResolvedValue(cachedConversation);
    vi.mocked(cacheDB.getMessages).mockResolvedValue([partialHistoryMessage]);
    vi.mocked(api.getConversationMessagesAfter).mockResolvedValue({
      messages: [],
      tombstones: [],
      transcript_generation: 1,
      server_message_tail: 2,
      has_older_messages: true,
    });
    vi.mocked(api.getConversationMessagesLatest).mockResolvedValue({
      messages: [partialHistoryMessage],
      tombstones: [],
      transcript_generation: 1,
      server_message_tail: 2,
      has_older_messages: true,
    });
    vi.mocked(api.getConversationMetaBySlug).mockResolvedValue({
      conversation: cachedConversation,
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });
    vi.mocked(api.getConversationBySlug).mockResolvedValue({
      conversation: replacementConversation,
      messages: [replacementMessage],
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });

    renderPage(cachedConversation);

    fireEvent.click(await screen.findByRole('button', { name: 'Load older messages' }));
    await waitFor(() => expect(api.getConversationBySlug).toHaveBeenCalledWith(slug));
    expect(await screen.findByTestId('history-loading')).toHaveTextContent('idle');
    expect(screen.queryByText('wrong conversation history')).not.toBeInTheDocument();
    expect(screen.getByText('keep this history visible')).toBeInTheDocument();
  });

  it('cold alignment fallback fetches full conversation by metadata id without re-resolving slug', async () => {
    const uuidRoute = '11111111-2222-4333-8444-555555555555';
    const uuidConversation = makeConversation({ id: uuidRoute, slug: 'uuid-fallback', transcript_generation: 7 });
    vi.mocked(cacheDB.getConversation).mockResolvedValue(uuidConversation);
    vi.mocked(cacheDB.getConversationBySlug).mockResolvedValue(null);
    vi.mocked(cacheDB.getMessages).mockResolvedValue([]);
    vi.mocked(api.getConversationMeta).mockResolvedValue({
      conversation: uuidConversation,
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });
    vi.mocked(api.getConversationMessagesLatest).mockRejectedValue(new MessageSliceAlignmentError('misaligned'));
    vi.mocked(api.getConversation).mockResolvedValue({
      conversation: uuidConversation,
      messages: [historyMessage],
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });

    renderPage(uuidConversation, uuidRoute);

    expect(await screen.findByText('keep this history visible')).toBeInTheDocument();
    await waitFor(() => expect(api.getConversation).toHaveBeenCalledWith(uuidRoute));
    expect(api.getConversationBySlug).not.toHaveBeenCalledWith(uuidRoute);
  });

  it('renders cached messages immediately and incrementally catches up newer messages without a full fetch', async () => {
    const cachedConversation = makeConversation({ transcript_generation: 7 });
    vi.mocked(cacheDB.getConversationBySlug).mockResolvedValue(cachedConversation);
    vi.mocked(cacheDB.getMessages).mockResolvedValue([historyMessage]);
    vi.mocked(cacheDB.getReplicaMeta).mockResolvedValue({
      conversationId,
      latestMessageSequenceId: 1,
      latestEventSequenceId: null,
      transcriptGeneration: 7,
      lastHydratedAt: '2024-01-01T00:00:02Z',
    });
    vi.mocked(cacheDB.getMaxMessageSequenceId).mockResolvedValue(1);
    vi.mocked(api.getConversationMessagesAfter).mockResolvedValue({
      messages: [catchUpMessage],
      tombstones: [],
      transcript_generation: 7,
      server_message_tail: 2,
      has_older_messages: false,
    });
    vi.mocked(api.getConversationMessagesLatest).mockResolvedValue({
      messages: [catchUpMessage],
      tombstones: [],
      transcript_generation: 7,
      server_message_tail: 2,
      has_older_messages: false,
    });
    vi.mocked(api.getConversationMetaBySlug).mockResolvedValue({
      conversation: cachedConversation,
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });
    vi.mocked(api.getConversationBySlug).mockClear();
    vi.mocked(api.getConversationBySlug).mockResolvedValue({
      conversation: cachedConversation,
      messages: [historyMessage],
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });

    render(
      <ConversationContext.Provider value={new ConversationStore()}>
        <DraftContext.Provider value={new DraftStore()}>
          <ConversationReadinessProvider>
            <MemoryRouter initialEntries={[`/c/${cachedConversation.slug}`]}>
              <Routes>
                <Route path="/c/:slug" element={<DesktopLayout><ConversationPage /></DesktopLayout>} />
              </Routes>
            </MemoryRouter>
          </ConversationReadinessProvider>
        </DraftContext.Provider>
      </ConversationContext.Provider>,
    );

    expect(await screen.findByText('keep this history visible')).toBeInTheDocument();
    expect(await screen.findByText('incremental catch-up arrived')).toBeInTheDocument();
    expect(api.getConversationMessagesAfter).toHaveBeenCalledWith(conversationId, 1, 200);
    expect(api.getConversationMessagesLatest).toHaveBeenCalledWith(conversationId, 50);
    expect(api.getConversationMetaBySlug).toHaveBeenCalledWith(cachedConversation.slug);
    expect(cacheDB.putMessages).toHaveBeenCalledWith([catchUpMessage]);
    await waitFor(() => {
      expect(cacheDB.putReplicaMeta).toHaveBeenCalledWith(
        expect.objectContaining({
          conversationId,
          latestMessageSequenceId: 2,
          latestEventSequenceId: null,
          transcriptGeneration: 7,
          lastHydratedAt: expect.any(String),
        }),
      );
    });
  });
});
