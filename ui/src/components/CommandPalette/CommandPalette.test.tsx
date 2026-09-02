import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { ConversationSearchWarmingError } from '../../api';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { CommandPalette } from './CommandPalette';
import { activeConversationFileRoot } from './fileRoot';
import { createFileSource } from './sources/FileSource';
import { FileExplorerContext } from '../FileExplorer/fileExplorerTypes';
import type { Conversation } from '../../api';

const mocks = vi.hoisted(() => ({
  searchConversationFiles: vi.fn(),
  searchConversationCode: vi.fn(),
  searchConversationContent: vi.fn(),
  openFile: vi.fn(),
  archiveConversation: vi.fn(),
  archiveChain: vi.fn(),
}));

vi.mock('../../api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../api')>();
  return {
    ...actual,
    api: {
      ...actual.api,
      searchConversationFiles: mocks.searchConversationFiles,
      searchConversationCode: mocks.searchConversationCode,
      searchConversationContent: mocks.searchConversationContent,
      archiveConversation: mocks.archiveConversation,
      archiveChain: mocks.archiveChain,
    },
  };
});

function makeConversation(overrides: Partial<Conversation> = {}): Conversation {
  return {
    id: 'conv-1',
    slug: 'active-conv',
    model: 'claude-sonnet-4-6',
    cwd: '/repo',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    message_count: 0,
    browser_session_active: false,
    terminal_uses_tmux: false,
    work_scope_key: 'conversation:conv-1',
    ...overrides,
  };
}

function LocationProbe() {
  const location = useLocation();
  return <output data-testid="location">{location.pathname}</output>;
}

function renderPalette(
  activeConversation: Conversation,
  conversations: readonly Conversation[] = [activeConversation],
) {
  Object.defineProperty(window, 'matchMedia', {
    writable: true,
    value: vi.fn().mockImplementation(() => ({
      matches: true,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    })),
  });

  return render(
    <MemoryRouter initialEntries={[`/c/${activeConversation.slug}`]}>
      <FileExplorerContext.Provider value={{
        openFile: mocks.openFile,
        activeFile: null,
        closeFile: vi.fn(),
        openFileState: null,
      }}>
        <CommandPalette
          conversations={conversations}
          activeConversation={activeConversation}
        />
        <LocationProbe />
      </FileExplorerContext.Provider>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  mocks.searchConversationFiles.mockReset();
  mocks.searchConversationCode.mockReset();
  mocks.searchConversationContent.mockReset();
  mocks.openFile.mockReset();
  mocks.archiveConversation.mockReset();
  mocks.archiveChain.mockReset();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('CommandPalette lifecycle availability', () => {
  it('does not offer Close for a continuation-linked row', () => {
    const root = makeConversation({ id: 'root-id', slug: 'root', continued_in_conv_id: 'leaf-id' });
    const leaf = makeConversation({ id: 'leaf-id', slug: 'leaf' });
    renderPalette(root, [root, leaf]);

    fireEvent.keyDown(window, { key: 'p', metaKey: true });
    fireEvent.change(screen.getByRole('textbox'), { target: { value: '> close' } });

    expect(screen.queryByText('Close Current Conversation')).toBeNull();
  });

  it('offers Close for a standalone conversation', () => {
    const standalone = makeConversation({ id: 'solo-id', slug: 'solo' });
    renderPalette(standalone, [standalone]);

    fireEvent.keyDown(window, { key: 'p', metaKey: true });
    fireEvent.change(screen.getByRole('textbox'), { target: { value: '> close' } });

    expect(screen.getByText('Close Current Conversation')).toBeInTheDocument();
  });

  it('offers Close for an Open canonical aggregate despite archived row drift', () => {
    const latest = makeConversation({ id: 'latest-id', slug: 'latest', archived: true });
    render(
      <MemoryRouter initialEntries={['/product-conversations/product-1']}>
        <FileExplorerContext.Provider value={{
          openFile: mocks.openFile,
          activeFile: null,
          closeFile: vi.fn(),
          openFileState: null,
        }}>
          <CommandPalette
            conversations={[latest]}
            productConversations={[{
              product_conversation_id: 'product-1',
              canonical_route: '/product-conversations/product-1',
              canonical_root: { transcript_row_id: 'root-id', slug: 'root', title: null },
              ordinary_lifecycle: 'open',
              latest_transcript_row_id: 'latest-id',
              updated_at: '2026-01-01T00:00:00Z',
              presentation: { kind: 'state', display_name: 'Product', presentation_mode: 'idle' },
            }]}
            activeConversation={latest}
          />
        </FileExplorerContext.Provider>
      </MemoryRouter>,
    );
    fireEvent.keyDown(window, { key: 'p', metaKey: true });
    fireEvent.change(screen.getByRole('textbox'), { target: { value: '> close' } });
    expect(screen.getByText('Close Current Conversation')).toBeInTheDocument();
  });

  it('closes an Open canonical aggregate from the active atom when drift hides its row', async () => {
    const latest = makeConversation({ id: 'latest-id', slug: 'latest', archived: true });
    render(
      <MemoryRouter initialEntries={['/product-conversations/product-1']}>
        <FileExplorerContext.Provider value={{ openFile: mocks.openFile, activeFile: null, closeFile: vi.fn(), openFileState: null }}>
          <CommandPalette
            conversations={[]}
            productConversations={[{
              product_conversation_id: 'product-1', canonical_route: '/product-conversations/product-1',
              canonical_root: { transcript_row_id: 'root-id', slug: 'root', title: null },
              ordinary_lifecycle: 'open', latest_transcript_row_id: 'latest-id',
              updated_at: '2026-01-01T00:00:00Z',
              presentation: { kind: 'state', display_name: 'Product', presentation_mode: 'idle' },
            }]}
            activeConversation={latest}
          />
        </FileExplorerContext.Provider>
      </MemoryRouter>,
    );

    fireEvent.keyDown(window, { key: 'p', metaKey: true });
    fireEvent.change(screen.getByRole('textbox'), { target: { value: '> close' } });
    fireEvent.click(screen.getByText('Close Current Conversation'));

    await waitFor(() => expect(mocks.archiveConversation).toHaveBeenCalledWith('latest-id'));
    expect(mocks.archiveChain).not.toHaveBeenCalled();
  });

  it('does not offer Close on a canonical History aggregate', () => {
    const latest = makeConversation({ id: 'latest-id', slug: 'latest', archived: false });
    render(
      <MemoryRouter initialEntries={['/product-conversations/product-1']}>
        <FileExplorerContext.Provider value={{ openFile: mocks.openFile, activeFile: null, closeFile: vi.fn(), openFileState: null }}>
          <CommandPalette
            conversations={[latest]}
            productConversations={[{
              product_conversation_id: 'product-1', canonical_route: '/product-conversations/product-1',
              canonical_root: { transcript_row_id: 'root-id', slug: 'root', title: null },
              ordinary_lifecycle: 'history', latest_transcript_row_id: 'latest-id',
              updated_at: '2026-01-01T00:00:00Z',
              presentation: { kind: 'state', display_name: 'Product', presentation_mode: 'idle' },
            }]}
            activeConversation={latest}
          />
        </FileExplorerContext.Provider>
      </MemoryRouter>,
    );
    fireEvent.keyDown(window, { key: 'p', metaKey: true });
    fireEvent.change(screen.getByRole('textbox'), { target: { value: '> close' } });
    expect(screen.queryByText('Close Current Conversation')).toBeNull();
  });


  it('does not offer Close for an archived conversation', () => {
    const archived = makeConversation({ id: 'history-id', slug: 'history', archived: true });
    renderPalette(archived, [archived]);

    fireEvent.keyDown(window, { key: 'p', metaKey: true });
    fireEvent.change(screen.getByRole('textbox'), { target: { value: '> close' } });

    expect(screen.queryByText('Close Current Conversation')).toBeNull();
  });
});

describe('CommandPalette file root', () => {
  it('prefers the active conversation worktree path over cwd', () => {
    expect(activeConversationFileRoot(makeConversation({
      cwd: '/repo',
      worktree_path: '/repo/.phoenix/worktrees/conv-1',
    }))).toBe('/repo/.phoenix/worktrees/conv-1');
  });

  it('falls back to cwd for direct conversations', () => {
    expect(activeConversationFileRoot(makeConversation({
      cwd: '/repo',
      worktree_path: null,
    }))).toBe('/repo');
  });

  it('returns no file root for archived conversations', () => {
    expect(activeConversationFileRoot(makeConversation({
      archived: true,
      cwd: '/repo',
      worktree_path: '/repo/.phoenix/worktrees/conv-1',
    }))).toBeNull();
  });

  it('searches files for the active conversation and opens results under worktree_path', async () => {
    const activeConversation = makeConversation({
      cwd: '/repo',
      worktree_path: '/repo/.phoenix/worktrees/conv-1',
    });
    mocks.searchConversationFiles.mockResolvedValue({
      items: [
        { path: 'src/main.rs', viewer: { kind: 'text', category: 'code' } },
        { path: 'assets/blob.zip', viewer: { kind: 'opaque' } },
      ],
    });
    mocks.searchConversationCode.mockResolvedValue({ items: [] });

    renderPalette(activeConversation);
    fireEvent.keyDown(window, { key: 'p', metaKey: true });
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'main' } });

    await waitFor(() => {
      expect(mocks.searchConversationFiles).toHaveBeenCalledWith(
        'conv-1',
        'main',
        50,
        expect.any(AbortSignal),
      );
    });

    const mainRow = await screen.findByText('main.rs');
    expect(screen.queryByText('blob.zip')).not.toBeInTheDocument();
    fireEvent.click(mainRow);

    expect(mocks.openFile).toHaveBeenCalledWith(
      '/repo/.phoenix/worktrees/conv-1/src/main.rs',
      '/repo/.phoenix/worktrees/conv-1',
      undefined,
    );
  });

  it('searches code for the active conversation and opens hits at the matched line', async () => {
    const activeConversation = makeConversation({
      cwd: '/repo',
      worktree_path: '/repo/.phoenix/worktrees/conv-1',
    });
    mocks.searchConversationFiles.mockResolvedValue({ items: [] });
    mocks.searchConversationCode.mockResolvedValue({
      items: [{
        path: 'src/main.rs',
        line_number: 42,
        line_text: 'originProduct := metricSourceToOriginProduct[source]',
        match_start: 17,
        match_end: 44,
      }],
    });

    renderPalette(activeConversation);
    fireEvent.keyDown(window, { key: 'p', metaKey: true });
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'metricSourceToOriginProduct' } });

    await waitFor(() => {
      expect(mocks.searchConversationCode).toHaveBeenCalledWith(
        'conv-1',
        'metricSourceToOriginProduct',
        50,
        expect.any(AbortSignal),
      );
    });

    expect(await screen.findByText('metricSourceToOriginProduct')).toBeInTheDocument();
    expect(screen.getByText('src/main.rs:42')).toBeInTheDocument();
    expect(screen.getByText('originProduct := metricSourceToOriginProduct[source]')).toBeInTheDocument();

    fireEvent.click(screen.getByText('metricSourceToOriginProduct'));

    expect(mocks.openFile).toHaveBeenCalledWith(
      '/repo/.phoenix/worktrees/conv-1/src/main.rs',
      '/repo/.phoenix/worktrees/conv-1',
      { kind: 'line', lineNumber: 42 },
    );
  });
});

describe('CommandPalette conversation scope', () => {
  it('shows content hits for c and does not search files or code', async () => {
    const activeConversation = makeConversation();
    mocks.searchConversationFiles.mockResolvedValue({ items: [{ path: 'c-file.ts', viewer: { kind: 'text', category: 'code' } }] });
    mocks.searchConversationCode.mockResolvedValue({ items: [{
      path: 'src/c.ts',
      line_number: 1,
      line_text: 'const c = true;',
      match_start: 6,
      match_end: 7,
    }] });
    mocks.searchConversationContent.mockResolvedValue({
      hits: [{
        conversation_id: 'conv-1',
        slug: 'active-conv',
        archived: false,
        message_id: 'msg-1',
        message_type: 'user',
        created_at: '2026-01-01T00:00:00Z',
        snippet: 'Need to fix the C scope search',
        score: 0.9,
      }],
    });

    renderPalette(activeConversation);
    fireEvent.keyDown(window, { key: 'p', metaKey: true });
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'c fix' } });

    expect(await screen.findByText('active-conv')).toBeInTheDocument();
    expect(screen.getByText('Need to fix the C scope search')).toBeInTheDocument();
    expect(mocks.searchConversationContent).toHaveBeenCalledWith('fix', 20, expect.any(AbortSignal));
    expect(mocks.searchConversationFiles).not.toHaveBeenCalled();
    expect(mocks.searchConversationCode).not.toHaveBeenCalled();
    expect(screen.queryByText('c-file.ts')).not.toBeInTheDocument();
  });

  it('removes global results immediately when conversation content scope is entered', async () => {
    const activeConversation = makeConversation();
    mocks.searchConversationFiles.mockResolvedValue({
      items: [{ path: 'src/main.rs', viewer: { kind: 'text', category: 'code' } }],
    });
    mocks.searchConversationCode.mockResolvedValue({ items: [] });
    mocks.searchConversationContent.mockResolvedValue({
      hits: [{
        conversation_id: 'conv-1',
        slug: 'active-conv',
        archived: false,
        message_id: 'msg-1',
        message_type: 'user',
        created_at: '2026-01-01T00:00:00Z',
        snippet: 'main appears in the conversation transcript',
        score: 0.7,
      }],
    });

    renderPalette(activeConversation);
    fireEvent.keyDown(window, { key: 'p', metaKey: true });
    const input = screen.getByRole('textbox');
    fireEvent.change(input, { target: { value: 'main' } });
    expect(await screen.findByText('main.rs')).toBeInTheDocument();

    fireEvent.change(input, { target: { value: 'c main' } });
    expect(screen.queryByText('main.rs')).not.toBeInTheDocument();
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(mocks.openFile).not.toHaveBeenCalled();
  });

  it('uses cs for slug search and navigates to the best fuzzy match on Enter', async () => {
    const activeConversation = makeConversation();
    const emojiConversation = makeConversation({
      id: 'conv-emoji',
      slug: 'emoji-search-improvements',
      updated_at: '2025-01-01T00:00:00Z',
    });
    const fuzzyConversation = makeConversation({
      id: 'conv-fuzzy',
      slug: 'extract-model-output',
      updated_at: '2026-02-01T00:00:00Z',
    });

    renderPalette(activeConversation, [activeConversation, fuzzyConversation, emojiConversation]);
    fireEvent.keyDown(window, { key: 'p', metaKey: true });
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'cs emo' } });

    const results = await screen.findAllByRole('button');
    expect(results[0]).toHaveTextContent('emoji-search-improvements');
    fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Enter' });

    expect(screen.getByTestId('location')).toHaveTextContent('/c/emoji-search-improvements');
  });

  it('returns to global search when the scope prefix is removed', async () => {
    const activeConversation = makeConversation();
    mocks.searchConversationFiles.mockResolvedValue({ items: [] });
    mocks.searchConversationCode.mockResolvedValue({ items: [] });
    mocks.searchConversationContent.mockResolvedValue({
      hits: [{
        conversation_id: 'conv-1',
        slug: 'active-conv',
        archived: false,
        message_id: 'msg-1',
        message_type: 'user',
        created_at: '2026-01-01T00:00:00Z',
        snippet: 'main appears in the conversation transcript',
        score: 0.7,
      }],
    });

    renderPalette(activeConversation);
    fireEvent.keyDown(window, { key: 'p', metaKey: true });
    const input = screen.getByRole('textbox');
    fireEvent.change(input, { target: { value: 'c main' } });
    expect(await screen.findByText('active-conv')).toBeInTheDocument();

    fireEvent.change(input, { target: { value: 'main' } });
    await waitFor(() => {
      expect(mocks.searchConversationFiles).toHaveBeenCalledWith(
        'conv-1',
        'main',
        50,
        expect.any(AbortSignal),
      );
    });
  });

  it('shows warming, error, and no-results states for content search', async () => {
    const activeConversation = makeConversation();
    renderPalette(activeConversation);
    fireEvent.keyDown(window, { key: 'p', metaKey: true });
    const input = screen.getByRole('textbox');

    mocks.searchConversationContent.mockRejectedValueOnce(new ConversationSearchWarmingError('Search index is warming'));
    fireEvent.change(input, { target: { value: 'c warm' } });
    expect(await screen.findByText('Search index is warming')).toBeInTheDocument();

    mocks.searchConversationContent.mockRejectedValueOnce(new Error('Search request failed'));
    fireEvent.change(input, { target: { value: 'c broken' } });
    expect(await screen.findByText('Search request failed')).toBeInTheDocument();

    mocks.searchConversationContent.mockResolvedValueOnce({ hits: [] });
    fireEvent.change(input, { target: { value: 'c none' } });
    expect(await screen.findByText('No conversation content results')).toBeInTheDocument();
  });

  it('does not debounce or call content search for an empty c query and shows guidance', async () => {
    const activeConversation = makeConversation();
    renderPalette(activeConversation);
    fireEvent.keyDown(window, { key: 'p', metaKey: true });

    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'c ' } });

    expect(await screen.findByText('Type a query to search conversation content')).toBeInTheDocument();
    expect(screen.queryByText('Waiting for more typing…')).not.toBeInTheDocument();
    expect(mocks.searchConversationContent).not.toHaveBeenCalled();
  });

  it('keeps cs empty on recent conversation slugs defaults', async () => {
    const activeConversation = makeConversation({ updated_at: '2026-01-01T00:00:00Z' });
    const recent = makeConversation({ id: 'conv-recent', slug: 'recent-conv', updated_at: '2026-02-01T00:00:00Z' });
    const older = makeConversation({ id: 'conv-older', slug: 'older-conv', updated_at: '2025-12-01T00:00:00Z' });

    renderPalette(activeConversation, [older, activeConversation, recent]);
    fireEvent.keyDown(window, { key: 'p', metaKey: true });
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'cs ' } });

    const results = await screen.findAllByRole('button');
    expect(results[0]).toHaveTextContent('recent-conv');
    expect(results[1]).toHaveTextContent('active-conv');
    expect(mocks.searchConversationContent).not.toHaveBeenCalled();
  });

  it('uses generic loading text for global search', async () => {
    const activeConversation = makeConversation();
    mocks.searchConversationFiles.mockImplementation(() => new Promise(() => {}));
    mocks.searchConversationCode.mockImplementation(() => new Promise(() => {}));

    renderPalette(activeConversation);
    fireEvent.keyDown(window, { key: 'p', metaKey: true });
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'main' } });

    expect(await screen.findByText('Searching…')).toBeInTheDocument();
    expect(screen.queryByText('Searching conversation content…')).not.toBeInTheDocument();
  });

  it('navigates archived content hits by slug and shows archived indicator', async () => {
    const activeConversation = makeConversation();
    mocks.searchConversationContent.mockResolvedValue({
      hits: [{
        conversation_id: 'conv-archived',
        slug: 'archived-hit',
        archived: true,
        message_id: 'msg-archived',
        message_type: 'agent',
        created_at: '2026-01-01T00:00:00Z',
        snippet: 'Archived transcript hit',
        score: 0.95,
      }],
    });

    renderPalette(activeConversation, [activeConversation, makeConversation({ id: 'conv-archived', slug: 'archived-hit', archived: true })]);
    fireEvent.keyDown(window, { key: 'p', metaKey: true });
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'c archived' } });

    const archivedBadge = (await screen.findAllByText('Archived'))[0];
    expect(archivedBadge?.closest('.cp-result-title-row')).toHaveTextContent('archived-hitArchived');
    fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Enter' });
    expect(screen.getByTestId('location')).toHaveTextContent('/c/archived-hit');
  });

  it('aborts content search before entering action mode', async () => {
    let resolveContent: ((value: {
      hits: Array<{
        conversation_id: string;
        slug: string;
        archived: boolean;
        message_id: string;
        message_type: string;
        created_at: string;
        snippet: string;
        score: number;
      }>;
    }) => void) | null = null;
    let contentSignal: AbortSignal | null = null;
    mocks.searchConversationContent.mockImplementation((_, __, signal?: AbortSignal) => {
      contentSignal = signal ?? null;
      return new Promise(resolve => {
        resolveContent = resolve;
      });
    });

    renderPalette(makeConversation());
    fireEvent.keyDown(window, { key: 'p', metaKey: true });
    const input = screen.getByRole('textbox');
    fireEvent.change(input, { target: { value: 'c pending' } });
    await waitFor(() => expect(mocks.searchConversationContent).toHaveBeenCalledOnce());

    fireEvent.change(input, { target: { value: '>new' } });
    expect(await screen.findByText('New Conversation')).toBeInTheDocument();
    const abortedContentSignal = contentSignal as AbortSignal | null;
    expect(abortedContentSignal?.aborted).toBe(true);

    const finishContent = resolveContent as ((value: { hits: Array<{
      conversation_id: string;
      slug: string;
      archived: boolean;
      message_id: string;
      message_type: string;
      created_at: string;
      snippet: string;
      score: number;
    }> }) => void) | null;
    finishContent?.({
      hits: [{
        conversation_id: 'conv-stale',
        slug: 'stale-content-hit',
        archived: false,
        message_id: 'msg-stale',
        message_type: 'user',
        created_at: '2026-01-01T00:00:00Z',
        snippet: 'Must not replace actions',
        score: 0.1,
      }],
    });

    await waitFor(() => expect(screen.queryByText('stale-content-hit')).not.toBeInTheDocument());
    expect(screen.getByText('New Conversation')).toBeInTheDocument();
  });

  it('does not restart content search when conversation polling refreshes props', async () => {
    let resolveContent: ((value: { hits: [] }) => void) | null = null;
    let searchSignal: AbortSignal | null = null;
    mocks.searchConversationContent.mockImplementation((_, __, signal?: AbortSignal) => {
      searchSignal = signal ?? null;
      return new Promise(resolve => {
        resolveContent = resolve as (value: { hits: [] }) => void;
      });
    });

    const conversation = makeConversation();
    const rendered = renderPalette(conversation);
    fireEvent.keyDown(window, { key: 'p', metaKey: true });
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'c pending' } });
    await waitFor(() => expect(mocks.searchConversationContent).toHaveBeenCalledOnce());

    rendered.rerender(
      <MemoryRouter initialEntries={[`/c/${conversation.slug}`]}>
        <FileExplorerContext.Provider value={{
          openFile: mocks.openFile,
          activeFile: null,
          closeFile: vi.fn(),
          openFileState: null,
        }}>
          <CommandPalette
            conversations={[{ ...conversation, updated_at: '2026-01-01T00:00:05Z' }]}
            activeConversation={conversation}
          />
          <LocationProbe />
        </FileExplorerContext.Provider>
      </MemoryRouter>,
    );

    await new Promise(resolve => setTimeout(resolve, 150));
    expect(mocks.searchConversationContent).toHaveBeenCalledOnce();
    const activeSignal = searchSignal as AbortSignal | null;
    expect(activeSignal?.aborted).toBe(false);
    const finishContent = resolveContent as ((value: { hits: [] }) => void) | null;
    await act(async () => {
      finishContent?.({ hits: [] });
    });
  });

  it('suppresses stale out-of-order content responses', async () => {
    const firstSignal = { current: null as AbortSignal | null };
    let firstReject: ((reason?: unknown) => void) | null = null;
    mocks.searchConversationContent
      .mockImplementationOnce((_, __, signal?: AbortSignal) => new Promise((_, reject) => {
        firstSignal.current = signal ?? null;
        firstReject = reject;
      }))
      .mockResolvedValueOnce({
        hits: [{
          conversation_id: 'conv-new',
          slug: 'new-hit',
          archived: false,
          message_id: 'msg-new',
          message_type: 'user',
          created_at: '2026-01-01T00:00:00Z',
          snippet: 'Newest result',
          score: 0.9,
        }],
      });

    const activeConversation = makeConversation();
    renderPalette(activeConversation);
    fireEvent.keyDown(window, { key: 'p', metaKey: true });
    const input = screen.getByRole('textbox');

    fireEvent.change(input, { target: { value: 'c old' } });
    await waitFor(() => expect(mocks.searchConversationContent).toHaveBeenCalledTimes(1));

    fireEvent.change(input, { target: { value: 'c new' } });
    await waitFor(() => expect(mocks.searchConversationContent).toHaveBeenCalledTimes(2));
    expect(firstSignal.current?.aborted).toBe(true);
    const rejectStaleRequest = firstReject as ((reason?: unknown) => void) | null;
    rejectStaleRequest?.(new DOMException('Aborted', 'AbortError'));

    expect(await screen.findByText('new-hit')).toBeInTheDocument();
    expect(screen.queryByText('old-hit')).not.toBeInTheDocument();
  });
});

describe('Conversation content source selection routing', () => {
  it('navigates content results through the registered source on click and Enter', async () => {
    const activeConversation = makeConversation();
    mocks.searchConversationContent.mockResolvedValue({
      hits: [{
        conversation_id: 'conv-2',
        slug: 'selected-via-source',
        archived: false,
        message_id: 'msg-2',
        message_type: 'agent',
        created_at: '2026-01-01T00:00:00Z',
        snippet: 'Select me',
        score: 0.8,
      }],
    });

    renderPalette(activeConversation, [activeConversation, makeConversation({ id: 'conv-2', slug: 'selected-via-source' })]);
    fireEvent.keyDown(window, { key: 'p', metaKey: true });
    const input = screen.getByRole('textbox');
    fireEvent.change(input, { target: { value: 'c selected' } });

    const result = await screen.findByText('selected-via-source');
    fireEvent.click(result);
    expect(screen.getByTestId('location')).toHaveTextContent('/c/selected-via-source');

    fireEvent.keyDown(window, { key: 'p', metaKey: true });
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'c selected' } });
    await screen.findByText('selected-via-source');
    fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Enter' });
    expect(screen.getByTestId('location')).toHaveTextContent('/c/selected-via-source');
  });
});

describe('FileSource', () => {
  it('opens selected relative paths under the root used for search', () => {
    const source = createFileSource(
      'conv-1',
      '/repo/.phoenix/worktrees/conv-1',
      mocks.openFile,
    );

    source.onSelect({
      id: 'src/main.rs',
      title: 'main.rs',
      category: 'Files',
      sourceId: 'files',
      metadata: 'src/main.rs',
    });

    expect(mocks.openFile).toHaveBeenCalledWith(
      '/repo/.phoenix/worktrees/conv-1/src/main.rs',
      '/repo/.phoenix/worktrees/conv-1',
    );
  });
});
