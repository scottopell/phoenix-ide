import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { ConversationPage } from './ConversationPage';
import { DesktopLayout } from '../components/DesktopLayout';
import { ConversationContext } from '../conversation/ConversationContext';
import { DraftContext } from '../conversation/DraftContext';
import { ConversationStore } from '../conversation';
import { DraftStore } from '../conversation/DraftStore';
import { api, type Conversation, type Message } from '../api';
import { ConversationReadinessProvider } from '../contexts/ConversationReadinessContext';
import { cacheDB } from '../cache';

const viewportFlags = vi.hoisted(() => ({ isDesktop: true, isWideDesktop: true }));

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
    syncConversations: vi.fn(() => Promise.resolve()),
    putConversation: vi.fn(() => Promise.resolve()),
    putMessages: vi.fn(() => Promise.resolve()),
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
    historyScrollCommand,
    olderHistoryError,
  }: {
    messages: Message[];
    hasOlderMessages?: boolean;
    onLoadOlderMessages?: () => void;
    loadingOlderMessages?: boolean;
    historyScrollCommand?: { kind: string; token: number } | null;
    olderHistoryError?: string | null;
  }) => (
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
        {historyScrollCommand ? `${historyScrollCommand.kind}:${historyScrollCommand.token}` : 'none'}
      </div>
      {olderHistoryError && <div role="alert">{olderHistoryError}</div>}
    </div>
  ),
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

function makeConversation(overrides: Partial<Conversation> = {}): Conversation {
  return {
    id: conversationId,
    slug,
    model: 'claude-3-5-sonnet',
    cwd: '/repo/project',
    created_at: '2024-01-01T00:00:00Z',
    updated_at: '2024-01-01T00:00:01Z',
    message_count: 1,
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

function renderPage(conversation: Conversation, routeSegment: string = conversation.slug) {
  const store = new ConversationStore();
  store.dispatch(conversation.slug, {
    type: 'set_initial_data',
    conversationId: conversation.id,
    conversation,
    messages: [{ ...historyMessage, conversation_id: conversation.id }],
    phase: { type: 'idle' },
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

  render(
    <ConversationContext.Provider value={store}>
      <DraftContext.Provider value={new DraftStore()}>
        <ConversationReadinessProvider>
          <MemoryRouter initialEntries={[`/c/${routeSegment}`]}>
            <Routes>
              <Route path="/c/:slug" element={<DesktopLayout><ConversationPage /></DesktopLayout>} />
            </Routes>
          </MemoryRouter>
        </ConversationReadinessProvider>
      </DraftContext.Provider>
    </ConversationContext.Provider>,
  );

  return { store };
}

afterEach(() => {
  viewportFlags.isDesktop = true;
  viewportFlags.isWideDesktop = true;
  vi.restoreAllMocks();
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

  it('preserves complete coverage when the cache already starts at the transcript beginning', async () => {
    const cachedConversation = makeConversation();
    vi.mocked(cacheDB.getConversationBySlug).mockResolvedValue(cachedConversation);
    vi.mocked(cacheDB.getMessages).mockResolvedValue([historyMessage, catchUpMessage]);
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
    expect(screen.queryByRole('button', { name: 'Load older messages' })).not.toBeInTheDocument();
  });

  it('fills a tail that grows during latest-window refresh before advancing replica coverage', async () => {
    const cachedConversation = makeConversation();
    const message3 = { ...catchUpMessage, message_id: 'm3', sequence_id: 3, content: [{ type: 'text', text: 'middle message' }] } as Message;
    const message4 = { ...catchUpMessage, message_id: 'm4', sequence_id: 4, content: [{ type: 'text', text: 'new tail message' }] } as Message;
    vi.mocked(cacheDB.getConversationBySlug).mockResolvedValue(cachedConversation);
    vi.mocked(cacheDB.getMessages).mockResolvedValue([historyMessage]);
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

  it('rejects metadata/latest-window generation mismatches instead of merging stale latest-window messages', async () => {
    const mismatchConversation = makeConversation({ transcript_generation: 2 });
    const staleLatestWindowMessage = {
      ...catchUpMessage,
      message_id: 'm-stale-latest-window',
      sequence_id: 2,
      content: [{ type: 'text', text: 'stale latest-window message' }],
    } as Message;

    vi.mocked(cacheDB.getConversationBySlug).mockResolvedValue(null);
    vi.mocked(cacheDB.getConversation).mockResolvedValue(null);
    vi.mocked(api.getConversationMetaBySlug).mockResolvedValue({
      conversation: mismatchConversation,
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });
    vi.mocked(api.getConversationMessagesLatest).mockResolvedValue({
      messages: [staleLatestWindowMessage],
      tombstones: [],
      transcript_generation: 1,
      server_message_tail: 2,
      has_older_messages: true,
    });

    vi.mocked(cacheDB.putReplicaMeta).mockClear();
    vi.mocked(cacheDB.putMessages).mockClear();

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

    expect(await screen.findByText('Conversation transcript changed while loading')).toBeInTheDocument();
    expect(screen.queryByText('stale latest-window message')).not.toBeInTheDocument();
    expect(cacheDB.putMessages).not.toHaveBeenCalledWith([staleLatestWindowMessage]);
    expect(cacheDB.putReplicaMeta).not.toHaveBeenCalled();
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

  it('renders cached messages immediately and incrementally catches up newer messages without a full fetch', async () => {
    const cachedConversation = makeConversation();
    vi.mocked(cacheDB.getConversationBySlug).mockResolvedValue(cachedConversation);
    vi.mocked(cacheDB.getMessages).mockResolvedValue([historyMessage]);
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
