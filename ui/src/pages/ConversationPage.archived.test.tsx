import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
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
      getConversationBySlug: vi.fn(),
      getConversationMessagesAfter: vi.fn(),
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

vi.mock('../hooks', async () => {
  const actual = await vi.importActual<typeof import('../hooks')>('../hooks');
  return {
    ...actual,
    useConnection: () => ({ state: 'connected', attempt: 0, nextRetryIn: null, retryNow: vi.fn() }),
    useIsDesktop: () => viewportFlags.isDesktop,
    useIsWideDesktop: () => viewportFlags.isWideDesktop,
  };
});

vi.mock('../components/ConversationNavStack', () => ({
  ConversationNavStack: ({ messages }: { messages: Message[] }) => (
    <div data-testid="message-history">
      {messages.map((message) => {
        const content = message.content as { text?: string } | { type?: string; text?: string }[];
        const rendered = Array.isArray(content)
          ? content.find((block) => block.type === 'text')?.text
          : content?.text;
        return <div key={message.message_id}>{rendered}</div>;
      })}
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

function renderPage(conversation: Conversation) {
  const store = new ConversationStore();
  store.dispatch(conversation.slug, {
    type: 'set_initial_data',
    conversationId: conversation.id,
    conversation,
    messages: [historyMessage],
    phase: { type: 'idle' },
    contextWindow: { used: 0 },
    transcriptGeneration: conversation.transcript_generation ?? 1,
  });

  vi.mocked(api.getConversationBySlug).mockResolvedValue({
    conversation,
    messages: [historyMessage],
    agent_working: false,
    presentation_mode: 'idle',
    context_window_size: 0,
  });

  render(
    <ConversationContext.Provider value={store}>
      <DraftContext.Provider value={new DraftStore()}>
        <ConversationReadinessProvider>
          <MemoryRouter initialEntries={[`/c/${conversation.slug}`]}>
            <Routes>
              <Route path="/c/:slug" element={<DesktopLayout><ConversationPage /></DesktopLayout>} />
            </Routes>
          </MemoryRouter>
        </ConversationReadinessProvider>
      </DraftContext.Provider>
    </ConversationContext.Provider>,
  );
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
