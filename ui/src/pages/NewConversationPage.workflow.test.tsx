import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor, act, fireEvent } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { NewConversationPage } from './NewConversationPage';
import { ConversationProvider } from '../conversation';
import { api } from '../api';

const originalFetch = globalThis.fetch;

const modelResponse = {
  models: [{ id: 'claude-3-5-sonnet', provider: 'anthropic', recommended: true }],
  default: 'claude-3-5-sonnet',
  llm_configured: true,
  credential_status: 'valid',
};

const branches = [
  { name: 'main', local: true, remote: true },
  { name: 'feature/demo', local: true, remote: true },
];

const task = {
  id: '27108',
  priority: 'p1',
  status: 'ready',
  slug: 'refine-new-workflows',
  path: '/repo/tasks/27108-p1-ready--refine-new-workflows.md',
};

vi.mock('../api', () => {
  class ExpansionError extends Error {
    detail: { error: string };

    constructor(error: string) {
      super(error);
      this.detail = { error };
    }
  }

  return {
    ExpansionError,
    api: {
      listModels: vi.fn(),
      getEnv: vi.fn(),
      validateCwd: vi.fn(),
      listDirectory: vi.fn(),
      listGitBranches: vi.fn(),
      getProjectTaskAvailability: vi.fn(),
      listProjectTasks: vi.fn(),
      createConversation: vi.fn(),
      listConversations: vi.fn().mockResolvedValue([]),
      listArchivedConversations: vi.fn().mockResolvedValue([]),
    },
  };
});

vi.mock('../cache', () => ({
  cacheDB: {
    getAllConversations: vi.fn().mockResolvedValue([]),
    syncConversations: vi.fn().mockResolvedValue(undefined),
    putConversation: vi.fn().mockResolvedValue(undefined),
  },
}));

function renderPage() {
  return render(
    <MemoryRouter>
      <ConversationProvider>
        <NewConversationPage />
      </ConversationProvider>
    </MemoryRouter>,
  );
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(r => { resolve = r; });
  return { promise, resolve };
}

async function settleValidation() {
  await act(async () => {
    await new Promise(resolve => setTimeout(resolve, 350));
  });
}

interface DraftStorageFailureOverrides {
  getItem?: (original: Storage, key: string) => string | null;
  setItem?: (original: Storage, key: string, value: string) => void;
  removeItem?: (original: Storage, key: string) => void;
}

function withDraftStorageFailure(overrides: DraftStorageFailureOverrides): () => void {
  const original = window.localStorage;
  const fake = {
    get length() { return original.length; },
    clear: () => original.clear(),
    key: (index: number) => original.key(index),
    getItem: (key: string) => overrides.getItem?.(original, key) ?? original.getItem(key),
    setItem: (key: string, value: string) => {
      if (overrides.setItem) {
        overrides.setItem(original, key, value);
      } else {
        original.setItem(key, value);
      }
    },
    removeItem: (key: string) => {
      if (overrides.removeItem) {
        overrides.removeItem(original, key);
      } else {
        original.removeItem(key);
      }
    },
  } satisfies Storage;
  Object.defineProperty(window, 'localStorage', { configurable: true, value: fake });
  return () => {
    Object.defineProperty(window, 'localStorage', { configurable: true, value: original });
  };
}

describe('/new workflow modes', () => {
  beforeEach(() => {
    localStorage.clear();
    localStorage.setItem('phoenix-last-cwd', '/repo');
    localStorage.setItem('phoenix-last-model', 'claude-3-5-sonnet');
    vi.mocked(api.listModels).mockResolvedValue(modelResponse as never);
    vi.mocked(api.getEnv).mockResolvedValue({ home_dir: '/home/user' });
    vi.mocked(api.validateCwd).mockResolvedValue({ valid: true, is_git: true });
    vi.mocked(api.listDirectory).mockResolvedValue({ entries: [] });
    vi.mocked(api.listGitBranches).mockResolvedValue({ branches, current: 'feature/demo', default_branch: 'main' });
    vi.mocked(api.getProjectTaskAvailability).mockResolvedValue({ available: true });
    vi.mocked(api.listProjectTasks).mockResolvedValue({ tasks: [task] });
    vi.mocked(api.createConversation).mockResolvedValue({
      id: 'c1',
      slug: 'conv-1',
      model: 'claude-3-5-sonnet',
      cwd: '/repo',
      created_at: '',
      updated_at: '',
      message_count: 1,
    } as never);
    globalThis.fetch = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ content: '# Task body' }) }) as never;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    vi.clearAllMocks();
  });

  it('preserves typed draft text across unmount and remount', async () => {
    const firstRender = renderPage();
    await settleValidation();

    fireEvent.change(screen.getAllByPlaceholderText('What would you like to work on?')[0]!, { target: { value: 'remember this draft' } });
    expect(localStorage.getItem('phoenix-new-conversation-draft')).toBe('remember this draft');

    firstRender.unmount();
    renderPage();

    expect(screen.getAllByPlaceholderText('What would you like to work on?')[0]).toHaveValue('remember this draft');
  });

  it('clears the persisted draft after successfully starting a conversation', async () => {
    vi.mocked(api.validateCwd).mockResolvedValue({ valid: true, is_git: false });
    const firstRender = renderPage();
    await settleValidation();

    fireEvent.change(screen.getAllByPlaceholderText('What would you like to work on?')[0]!, { target: { value: 'send and clear me' } });
    expect(localStorage.getItem('phoenix-new-conversation-draft')).toBe('send and clear me');
    fireEvent.click(screen.getAllByRole('button', { name: 'Send' })[0]!);

    await waitFor(() => expect(screen.getAllByPlaceholderText('What would you like to work on?')[0]).toHaveValue(''));
    expect(localStorage.getItem('phoenix-new-conversation-draft')).toBeNull();

    firstRender.unmount();
    renderPage();

    expect(screen.getAllByPlaceholderText('What would you like to work on?')[0]).toHaveValue('');
  });

  it('shows a stable acknowledgement while create is pending and preserves the draft', async () => {
    vi.mocked(api.validateCwd).mockResolvedValue({ valid: true, is_git: false });
    vi.mocked(api.createConversation).mockImplementation(() => new Promise(() => {}));
    renderPage();
    await settleValidation();

    fireEvent.change(screen.getAllByPlaceholderText('What would you like to work on?')[0]!, { target: { value: 'slow mobile request' } });
    fireEvent.click(screen.getAllByRole('button', { name: 'Send' })[0]!);

    await screen.findAllByText('Creating conversation…');
    expect(screen.getAllByRole('button', { name: 'Send' })[0]).toBeDisabled();
    expect(screen.queryByRole('button', { name: /Creating/ })).not.toBeInTheDocument();
    expect(screen.getAllByPlaceholderText('What would you like to work on?')[0]).toHaveValue('slow mobile request');
    expect(localStorage.getItem('phoenix-new-conversation-draft')).toBe('slow mobile request');

  });

  it('restores the interactive composer and keeps the draft when create fails', async () => {
    vi.mocked(api.validateCwd).mockResolvedValue({ valid: true, is_git: false });
    vi.mocked(api.createConversation).mockRejectedValue(new Error('network dropped'));
    renderPage();
    await settleValidation();

    fireEvent.change(screen.getAllByPlaceholderText('What would you like to work on?')[0]!, { target: { value: 'retry this later' } });
    fireEvent.click(screen.getAllByRole('button', { name: 'Send' })[0]!);

    await screen.findAllByText('network dropped');
    expect(screen.getAllByRole('button', { name: 'Send' })[0]).toBeEnabled();
    expect(screen.getAllByPlaceholderText('What would you like to work on?')[0]).toHaveValue('retry this later');
    expect(localStorage.getItem('phoenix-new-conversation-draft')).toBe('retry this later');
    expect(screen.queryAllByText('Creating conversation…')).toHaveLength(0);
  });

  it('falls back to an empty draft when persisted draft storage cannot be read', async () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const restoreStorage = withDraftStorageFailure({
      getItem: (original: Storage, key: string) => {
        if (key === 'phoenix-new-conversation-draft') throw new Error('storage disabled');
        return original.getItem(key);
      },
    });

    try {
      renderPage();

      expect(screen.getAllByPlaceholderText('What would you like to work on?')[0]).toHaveValue('');
      expect(warnSpy).toHaveBeenCalledWith('Error reading new conversation draft from localStorage:', expect.any(Error));
    } finally {
      restoreStorage();
      warnSpy.mockRestore();
    }
  });

  it('keeps the composer usable when persisted draft storage cannot be written', async () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const restoreStorage = withDraftStorageFailure({
      setItem: (original: Storage, key: string, value: string) => {
        if (key === 'phoenix-new-conversation-draft') throw new Error('quota exceeded');
        return original.setItem(key, value);
      },
    });

    try {
      renderPage();
      await settleValidation();

      fireEvent.change(screen.getAllByPlaceholderText('What would you like to work on?')[0]!, { target: { value: 'still editable' } });

      expect(screen.getAllByPlaceholderText('What would you like to work on?')[0]).toHaveValue('still editable');
      await waitFor(() => expect(warnSpy).toHaveBeenCalledWith('Error saving new conversation draft to localStorage:', expect.any(Error)));
    } finally {
      restoreStorage();
      warnSpy.mockRestore();
    }
  });

  it('still clears the mounted composer when persisted draft clearing fails after send', async () => {
    vi.mocked(api.validateCwd).mockResolvedValue({ valid: true, is_git: false });
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const restoreStorage = withDraftStorageFailure({
      removeItem: (original: Storage, key: string) => {
        if (key === 'phoenix-new-conversation-draft') throw new Error('storage disabled');
        return original.removeItem(key);
      },
    });

    try {
      renderPage();
      await settleValidation();

      fireEvent.change(screen.getAllByPlaceholderText('What would you like to work on?')[0]!, { target: { value: 'clear state anyway' } });
      fireEvent.click(screen.getAllByRole('button', { name: 'Send' })[0]!);

      await waitFor(() => expect(api.createConversation).toHaveBeenCalled());
      await waitFor(() => expect(screen.getAllByPlaceholderText('What would you like to work on?')[0]).toHaveValue(''));
      expect(warnSpy).toHaveBeenCalledWith('Error clearing new conversation draft from localStorage:', expect.any(Error));
    } finally {
      restoreStorage();
      warnSpy.mockRestore();
    }
  });

  it('shows only direct workflow for non-git directories and submits direct mode', async () => {
    vi.mocked(api.validateCwd).mockResolvedValue({ valid: true, is_git: false });
    renderPage();

    await settleValidation();
    expect(screen.queryByText('Workflow')).not.toBeInTheDocument();
    expect(screen.queryAllByText('Work in this folder')).toHaveLength(0);
    expect(screen.queryAllByText('Chat in a fresh worktree')).toHaveLength(0);

    fireEvent.change(screen.getAllByPlaceholderText('What would you like to work on?')[0]!, { target: { value: 'hello' } });
    fireEvent.click(screen.getAllByRole('button', { name: 'Send' })[0]!);

    await waitFor(() => expect(api.createConversation).toHaveBeenCalled());
    expect(api.createConversation).toHaveBeenCalledWith(
      '/repo',
      'hello',
      expect.any(String),
      'claude-3-5-sonnet',
      [],
      'direct',
      null,
      undefined,
      undefined,
      [],
      expect.any(String),
    );
    expect(api.listGitBranches).not.toHaveBeenCalled();
  });

  it('submits the metadata-selected fresh worktree without requiring branch reselection', async () => {
    renderPage();

    await settleValidation();
    await screen.findAllByText('Chat in a fresh worktree');
    await waitFor(() => expect(api.listGitBranches).toHaveBeenCalledWith('/repo'));

    fireEvent.change(screen.getAllByPlaceholderText('What would you like to work on?')[0]!, { target: { value: 'wake up default' } });
    await waitFor(() => expect(screen.getAllByRole('button', { name: 'Send' })[0]).toBeEnabled());
    fireEvent.click(screen.getAllByRole('button', { name: 'Send' })[0]!);

    await waitFor(() => expect(api.createConversation).toHaveBeenCalled());
    expect(api.createConversation).toHaveBeenCalledWith(
      '/repo',
      'wake up default',
      expect.any(String),
      'claude-3-5-sonnet',
      [],
      'managed',
      'main',
      undefined,
      undefined,
      [],
      expect.any(String),
    );
    expect(screen.queryByText('Pick a Git branch to start from.')).not.toBeInTheDocument();
    expect(screen.queryByText('Pick a Git starting point')).not.toBeInTheDocument();
  });

  it('submits plan-from-branch as managed mode with the default base branch', async () => {
    renderPage();

    await settleValidation();
    await screen.findAllByText('Chat in a fresh worktree');
    await waitFor(() => expect(api.listGitBranches).toHaveBeenCalledWith('/repo'));
    fireEvent.click(screen.getAllByText('Chat in a fresh worktree')[0]!);
    fireEvent.change(screen.getAllByPlaceholderText('What would you like to work on?')[0]!, { target: { value: 'please plan' } });
    fireEvent.click(screen.getAllByRole('button', { name: 'Send' })[0]!);

    await waitFor(() => expect(api.createConversation).toHaveBeenCalled());
    expect(api.createConversation).toHaveBeenCalledWith(
      '/repo',
      'please plan',
      expect.any(String),
      'claude-3-5-sonnet',
      [],
      'managed',
      'main',
      undefined,
      undefined,
      [],
      expect.any(String),
    );
  });
  it('submits continue-branch as branch mode with the selected branch', async () => {
    renderPage();

    await settleValidation();
    await screen.findAllByText('Chat in a specific branch');
    await waitFor(() => expect(api.listGitBranches).toHaveBeenCalledWith('/repo'));
    fireEvent.click(screen.getAllByText('Chat in a specific branch')[0]!);
    fireEvent.focus(screen.getAllByDisplayValue('main')[0]!);
    fireEvent.click(screen.getAllByText('feature/demo (current)')[0]!);
    fireEvent.change(screen.getAllByPlaceholderText('What would you like to work on?')[0]!, { target: { value: 'continue it' } });
    fireEvent.click(screen.getAllByRole('button', { name: 'Send' })[0]!);

    await waitFor(() => expect(api.createConversation).toHaveBeenCalled());
    expect(api.createConversation).toHaveBeenCalledWith(
      '/repo',
      'continue it',
      expect.any(String),
      'claude-3-5-sonnet',
      [],
      'branch',
      'feature/demo',
      undefined,
      undefined,
      [],
      expect.any(String),
    );
  });

  it('resets stale branch selection after cwd changes', async () => {
    renderPage();

    await settleValidation();
    await screen.findAllByText('Chat in a specific branch');
    await waitFor(() => expect(api.listGitBranches).toHaveBeenCalledWith('/repo'));
    fireEvent.click(screen.getAllByText('Chat in a specific branch')[0]!);
    fireEvent.focus(screen.getAllByDisplayValue('main')[0]!);
    fireEvent.click(screen.getAllByText('feature/demo (current)')[0]!);

    vi.mocked(api.listGitBranches).mockImplementation(async (requestedCwd: string) => {
      if (requestedCwd === '/repo-two') {
        return {
          branches: [{ name: 'trunk', local: true, remote: true }],
          current: 'trunk',
          default_branch: 'trunk',
        };
      }
      return { branches, current: 'feature/demo', default_branch: 'main' };
    });
    fireEvent.change(screen.getAllByDisplayValue('/repo')[0]!, { target: { value: '/repo-two' } });
    await settleValidation();
    await waitFor(() => expect(api.listGitBranches).toHaveBeenCalledWith('/repo-two'));

    fireEvent.change(screen.getAllByPlaceholderText('What would you like to work on?')[0]!, { target: { value: 'after switch' } });
    fireEvent.click(screen.getAllByRole('button', { name: 'Send' })[0]!);

    await waitFor(() => expect(api.createConversation).toHaveBeenCalled());
    expect(api.createConversation).toHaveBeenCalledWith(
      '/repo-two',
      'after switch',
      expect.any(String),
      'claude-3-5-sonnet',
      [],
      'managed',
      'trunk',
      undefined,
      undefined,
      [],
      expect.any(String),
    );
  });

  it('defaults a different git repo back to fresh worktree after a direct choice', async () => {
    renderPage();

    await settleValidation();
    await screen.findAllByText('Work in this folder');
    fireEvent.click(screen.getAllByText('Work in this folder')[0]!);

    vi.mocked(api.listGitBranches).mockImplementation(async (requestedCwd: string) => {
      if (requestedCwd === '/repo-two') {
        return {
          branches: [{ name: 'trunk', local: true, remote: true }],
          current: 'trunk',
          default_branch: 'trunk',
        };
      }
      return { branches, current: 'feature/demo', default_branch: 'main' };
    });
    fireEvent.change(screen.getAllByDisplayValue('/repo')[0]!, { target: { value: '/repo-two' } });
    await settleValidation();
    await waitFor(() => expect(api.listGitBranches).toHaveBeenCalledWith('/repo-two'));

    fireEvent.change(screen.getAllByPlaceholderText('What would you like to work on?')[0]!, { target: { value: 'fresh repo' } });
    fireEvent.click(screen.getAllByRole('button', { name: 'Send' })[0]!);

    await waitFor(() => expect(api.createConversation).toHaveBeenCalled());
    expect(api.createConversation).toHaveBeenCalledWith(
      '/repo-two',
      'fresh repo',
      expect.any(String),
      'claude-3-5-sonnet',
      [],
      'managed',
      'trunk',
      undefined,
      undefined,
      [],
      expect.any(String),
    );
  });

  it('shows task workflow loading before availability settles without fetching the full list', async () => {
    vi.mocked(api.getProjectTaskAvailability).mockImplementation(() => new Promise(() => {}));
    renderPage();

    await settleValidation();
    await screen.findAllByText('Loading tasks...');
    expect(screen.getAllByText('Start from a task').length).toBeGreaterThan(0);
    expect(api.listProjectTasks).not.toHaveBeenCalled();
  });

  it('omits task workflow when task availability is absent', async () => {
    vi.mocked(api.getProjectTaskAvailability).mockResolvedValue({ available: false });
    renderPage();

    await settleValidation();
    await screen.findAllByText('Chat in a fresh worktree');
    await waitFor(() => expect(api.getProjectTaskAvailability).toHaveBeenCalledWith('/repo'));

    expect(screen.queryAllByText('Start from a task')).toHaveLength(0);
    expect(api.listProjectTasks).not.toHaveBeenCalled();
    expect(screen.getAllByText('Chat in a specific branch').length).toBeGreaterThan(0);
    expect(screen.getAllByText('Work in this folder').length).toBeGreaterThan(0);
  });

  it('lazy-loads tasks and does not expose task base branch controls', async () => {
    renderPage();

    await settleValidation();
    await screen.findAllByText('Start from a task');
    expect(api.listProjectTasks).not.toHaveBeenCalled();
    fireEvent.click(screen.getAllByText('Start from a task')[0]!);

    await waitFor(() => expect(api.listProjectTasks).toHaveBeenCalledWith('/repo'));
    expect(screen.queryByText('Base branch for planning')).not.toBeInTheDocument();
    await screen.findAllByText('27108 · refine-new-workflows');
  });

  it('filters tasks by number or slug before pagination', async () => {
    const taskList = Array.from({ length: 10 }, (_, index) => ({
      ...task,
      id: String(10000 + index),
      slug: `filler-${index}`,
      path: `/repo/tasks/${10000 + index}-p2-ready--filler-${index}.md`,
    })).concat({
      ...task,
      id: '07004',
      slug: 'merged-target-task',
      path: '/repo/tasks/07004-p1-ready--merged-target-task.md',
    });
    vi.mocked(api.listProjectTasks).mockResolvedValue({ tasks: taskList });
    renderPage();

    await settleValidation();
    fireEvent.click(screen.getAllByText('Start from a task')[0]!);
    await screen.findAllByPlaceholderText('Search tasks by number or name...');
    fireEvent.change(screen.getAllByPlaceholderText('Search tasks by number or name...')[0]!, { target: { value: '07004' } });

    expect((await screen.findAllByText('07004 · merged-target-task')).length).toBeGreaterThan(0);
  });

  it('does not let task workflow submit notes without a selected task', async () => {
    const userTaskList = [{ ...task, status: 'done' }];
    vi.mocked(api.listProjectTasks).mockResolvedValue({ tasks: userTaskList });
    renderPage();

    await settleValidation();
    fireEvent.click(screen.getAllByText('Start from a task')[0]!);
    await screen.findAllByText('No active tasks found.');
    fireEvent.change(screen.getAllByPlaceholderText('Optional notes for this task…')[0]!, { target: { value: 'notes only' } });
    expect(screen.getAllByRole('button', { name: 'Send' })[0]).toBeDisabled();
  });

  it('ignores a stale lazy task response after cwd changes', async () => {
    const repoOneTasks = deferred<{ tasks: typeof task[] }>();
    const repoTwoTask = {
      ...task,
      id: '07004',
      slug: 'repo-two-task',
      path: '/repo-two/tasks/07004-p1-ready--repo-two-task.md',
    };
    vi.mocked(api.listProjectTasks).mockImplementation((cwd: string) => {
      if (cwd === '/repo') return repoOneTasks.promise;
      return Promise.resolve({ tasks: [repoTwoTask] });
    });
    renderPage();

    await settleValidation();
    fireEvent.click(screen.getAllByText('Start from a task')[0]!);
    await waitFor(() => expect(api.listProjectTasks).toHaveBeenCalledWith('/repo'));

    fireEvent.change(screen.getAllByDisplayValue('/repo')[0]!, { target: { value: '/repo-two' } });
    await settleValidation();
    await waitFor(() => expect(api.getProjectTaskAvailability).toHaveBeenCalledWith('/repo-two'));

    await act(async () => {
      repoOneTasks.resolve({ tasks: [task] });
      await Promise.resolve();
    });
    expect(screen.queryByText('27108 · refine-new-workflows')).not.toBeInTheDocument();

    fireEvent.click(screen.getAllByText('Start from a task')[0]!);
    await waitFor(() => expect(api.listProjectTasks).toHaveBeenCalledWith('/repo-two'));
    expect((await screen.findAllByText('07004 · repo-two-task')).length).toBeGreaterThan(0);
  });

  it('submits task workflow with propose-task prompt and managed mode', async () => {
    vi.mocked(api.listProjectTasks).mockResolvedValue({
      tasks: [{ ...task, source_ref: 'origin/main', content: '# Remote task body\n' }],
    });
    renderPage();

    await settleValidation();
    await screen.findAllByText('Start from a task');
    fireEvent.click(screen.getAllByText('Start from a task')[0]!);
    await waitFor(() => expect(api.listProjectTasks).toHaveBeenCalledWith('/repo'));
    fireEvent.click(screen.getAllByText('27108 · refine-new-workflows')[0]!);
    expect((await screen.findAllByText('Remote task body')).length).toBeGreaterThan(0);
    fireEvent.change(screen.getAllByPlaceholderText('Optional notes for this task…')[0]!, { target: { value: 'extra notes' } });
    fireEvent.click(screen.getAllByRole('button', { name: 'Send' })[0]!);

    await waitFor(() => expect(api.createConversation).toHaveBeenCalled());
    const [, text,,,, mode, baseBranch,,,, checkoutRef] = vi.mocked(api.createConversation).mock.calls[0]!;
    expect(mode).toBe('managed');
    expect(baseBranch).toBe('main');
    expect(checkoutRef).toBe('origin/main');
    expect(text).toContain('Call the propose_task tool');
    expect(text).toContain('tasks/27108-p1-ready--refine-new-workflows.md');
    expect(text).toContain('extra notes');
  });
});
