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
  gateway_status: 'healthy',
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

vi.mock('../api', () => ({
  api: {
    listModels: vi.fn(),
    getEnv: vi.fn(),
    validateCwd: vi.fn(),
    listDirectory: vi.fn(),
    listGitBranches: vi.fn(),
    listProjectTasks: vi.fn(),
    createConversation: vi.fn(),
    listConversations: vi.fn().mockResolvedValue([]),
    listArchivedConversations: vi.fn().mockResolvedValue([]),
  },
}));

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

async function settleValidation() {
  await act(async () => {
    await new Promise(resolve => setTimeout(resolve, 350));
  });
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

    await waitFor(() => expect(api.createConversation).toHaveBeenCalled());
    expect(localStorage.getItem('phoenix-new-conversation-draft')).toBeNull();

    firstRender.unmount();
    renderPage();

    expect(screen.getAllByPlaceholderText('What would you like to work on?')[0]).toHaveValue('');
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
    );
  });

  it('omits task workflow when taskmd discovery finds no active tasks', async () => {
    vi.mocked(api.listProjectTasks).mockResolvedValue({ tasks: [] });
    renderPage();

    await settleValidation();
    await screen.findAllByText('Chat in a fresh worktree');
    await waitFor(() => expect(api.listProjectTasks).toHaveBeenCalledWith('/repo'));

    expect(screen.queryAllByText('Start from a task')).toHaveLength(0);
    expect(screen.getAllByText('Chat in a specific branch').length).toBeGreaterThan(0);
    expect(screen.getAllByText('Work in this folder').length).toBeGreaterThan(0);
  });

  it('does not let task workflow submit notes without a selected task', async () => {
    const userTaskList = [{ ...task, status: 'done' }];
    vi.mocked(api.listProjectTasks).mockResolvedValue({ tasks: userTaskList });
    renderPage();

    await settleValidation();
    await screen.findAllByText('Chat in a fresh worktree');
    expect(screen.queryAllByText('Start from a task')).toHaveLength(0);
  });

  it('submits task workflow with propose-task prompt and managed mode', async () => {
    renderPage();

    await settleValidation();
    await screen.findAllByText('Start from a task');
    await waitFor(() => expect(api.listProjectTasks).toHaveBeenCalledWith('/repo'));
    fireEvent.click(screen.getAllByText('Start from a task')[0]!);
    fireEvent.click(screen.getAllByText('27108 · refine-new-workflows')[0]!);
    fireEvent.change(screen.getAllByPlaceholderText('Optional notes for this task…')[0]!, { target: { value: 'extra notes' } });
    fireEvent.click(screen.getAllByRole('button', { name: 'Send' })[0]!);

    await waitFor(() => expect(api.createConversation).toHaveBeenCalled());
    const [, text,,,, mode, baseBranch] = vi.mocked(api.createConversation).mock.calls[0]!;
    expect(mode).toBe('managed');
    expect(baseBranch).toBe('main');
    expect(text).toContain('Call the propose_task tool');
    expect(text).toContain('tasks/27108-p1-ready--refine-new-workflows.md');
    expect(text).toContain('extra notes');
  });
});
