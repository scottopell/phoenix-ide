import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { CommandPalette } from './CommandPalette';
import { activeConversationFileRoot } from './fileRoot';
import { createFileSource } from './sources/FileSource';
import { FileExplorerContext } from '../FileExplorer/fileExplorerTypes';
import type { Conversation } from '../../api';

const mocks = vi.hoisted(() => ({
  searchConversationFiles: vi.fn(),
  searchConversationCode: vi.fn(),
  openFile: vi.fn(),
}));

vi.mock('../../api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../api')>();
  return {
    ...actual,
    api: {
      ...actual.api,
      searchConversationFiles: mocks.searchConversationFiles,
      searchConversationCode: mocks.searchConversationCode,
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

function renderPalette(activeConversation: Conversation) {
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
          conversations={[activeConversation]}
          activeConversation={activeConversation}
        />
      </FileExplorerContext.Provider>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  mocks.searchConversationFiles.mockReset();
  mocks.searchConversationCode.mockReset();
  mocks.openFile.mockReset();
});

afterEach(() => {
  vi.restoreAllMocks();
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

  it('searches files for the active conversation and opens results under worktree_path', async () => {
    const activeConversation = makeConversation({
      cwd: '/repo',
      worktree_path: '/repo/.phoenix/worktrees/conv-1',
    });
    mocks.searchConversationFiles.mockResolvedValue({
      items: [{ path: 'src/main.rs', viewer: { kind: 'text', category: 'code' } }],
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

    fireEvent.click(await screen.findByText('main.rs'));

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
      metadata: 'src/main.rs',
    });

    expect(mocks.openFile).toHaveBeenCalledWith(
      '/repo/.phoenix/worktrees/conv-1/src/main.rs',
      '/repo/.phoenix/worktrees/conv-1',
    );
  });
});
