// FileExplorerPanel detail navigation owns Tasks/Skills panel UI state. These
// tests lock the Back path to restore the same expanded group context instead
// of remounting both sections from defaults.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { FileExplorerPanel } from './FileExplorerPanel';
import { FileExplorerProvider } from './FileExplorerContext';
import { ViewerSlotProvider } from '../../contexts/ViewerSlotContext';
import { ConversationProvider } from '../../conversation';
import { api } from '../../api';
import type { SkillEntry, TaskEntry } from '../../api';

vi.mock('./FileTree', () => ({
  FileTree: ({ refreshKey }: { refreshKey?: number }) => (
    <div data-testid="file-tree" data-refresh-key={refreshKey} />
  ),
}));

vi.mock('../McpStatusPanel', () => ({
  McpStatusPanel: () => <div data-testid="mcp-panel" />,
}));

vi.mock('../WorkScopePanel', () => ({
  WorkScopeSection: () => <div data-testid="work-scope" />,
}));

vi.mock('../useWorkScopeSeed', () => ({
  useSeededLiveCount: () => 0,
}));

vi.mock('../workScopeHelpers', () => ({
  workScopeLiveCount: () => 0,
}));

vi.mock('../../api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../api')>();
  return {
    ...actual,
    api: {
      ...actual.api,
      getConversationTaskCount: vi.fn(),
      listConversationTasks: vi.fn(),
      listConversationSkills: vi.fn(),
      listConversations: vi.fn().mockResolvedValue([]),
      getConversationGitStatus: vi.fn(),
    },
  };
});

const tasks: TaskEntry[] = [
  task('10001', 'ready', 'ready-one'),
  task('10002', 'blocked', 'blocked-one'),
  task('10003', 'done', 'done-one'),
];

const skills: SkillEntry[] = [
  skill('builtin-one', 'builtin', '/builtin/skills/builtin-one/SKILL.md'),
  skill('project-one', 'project', '/repo/project/.agents/skills/project-one/SKILL.md'),
];

function task(id: string, status: string, slug: string): TaskEntry {
  return { id, priority: 'p2', status, slug, path: `/repo/tasks/${id}-p2-${status}--${slug}.md` };
}

function skill(name: string, source: string, path: string): SkillEntry {
  return { name, description: `${name} description`, source, path };
}

function renderPanel(conversationId = 'conv-1', canOpenWorkspaceDiff = true, collapsed = false) {
  return render(
    <MemoryRouter initialEntries={['/c/slug']}>
      <ConversationProvider>
        <ViewerSlotProvider scopeKey={conversationId} browserSessionActive={false}>
          <FileExplorerProvider>
            <FileExplorerPanel
              collapsed={collapsed}
              onToggle={() => {}}
              rootPath="/repo"
              conversationId={conversationId}
              showToast={() => {}}
              showError={() => {}}
              branchName="main"
              activeSlug="slug"
              canOpenWorkspaceDiff={canOpenWorkspaceDiff}
            />
          </FileExplorerProvider>
        </ViewerSlotProvider>
      </ConversationProvider>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.mocked(api.getConversationTaskCount).mockResolvedValue({ active: 2, closed: 1, blocked: 1, current: false });
  vi.mocked(api.listConversationTasks).mockResolvedValue({ tasks });
  vi.mocked(api.listConversationSkills).mockResolvedValue({ skills });
  vi.mocked(api.getConversationGitStatus).mockResolvedValue({ kind: 'non_git' });
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue({
    ok: true,
    json: () => Promise.resolve({ kind: 'text', content: '# detail' }),
  }));
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

describe('FileExplorerPanel grounding detail navigation', () => {
  it('renders only non-empty git status groups', async () => {
    vi.mocked(api.getConversationGitStatus).mockResolvedValue({
      kind: 'snapshot',
      checkout_status: { kind: 'named_branch', branch_name: 'feature', head_oid: 'abc123', remote_status: { kind: 'no_known' } },
      counts: { changed_paths: 2, staged_paths: 0, unstaged_paths: 1, untracked_paths: 1, conflicted_paths: 0 },
      changed_paths: [],
    });
    renderPanel();

    expect(await screen.findByText('Changes not staged for commit')).toBeInTheDocument();
    expect(screen.getByText('feature')).toBeInTheDocument();
    expect(screen.queryByText('On branch feature')).not.toBeInTheDocument();
    expect(screen.getByText('Untracked files')).toBeInTheDocument();
    expect(screen.queryByText('Changes to be committed')).not.toBeInTheDocument();
    expect(screen.queryByText('Unmerged paths')).not.toBeInTheDocument();
    const gitHeader = screen.getByText('Git').closest('button');
    expect(gitHeader).not.toHaveTextContent('changed');
    expect(screen.getByRole('button', { name: 'Open Git diff' })).toBeInTheDocument();
  });

  it('shows checkout and dirty state in the collapsed Git header only', async () => {
    vi.mocked(api.getConversationGitStatus).mockResolvedValue({
      kind: 'snapshot',
      checkout_status: { kind: 'named_branch', branch_name: 'feature', head_oid: 'abc123', remote_status: { kind: 'no_known' } },
      counts: { changed_paths: 2, staged_paths: 0, unstaged_paths: 1, untracked_paths: 1, conflicted_paths: 0 },
      changed_paths: [],
    });
    renderPanel();
    await screen.findByText('Changes not staged for commit');

    expect(screen.getByText('Git').closest('button')).not.toHaveTextContent('feature · 2 changed');
    fireEvent.click(screen.getByText('Git').closest('button')!);
    expect(screen.getByText('Git').closest('button')).toHaveTextContent('feature · 2 changed · 1 unstaged · 1 untracked');
  });

  it('shows the locally known upstream relationship', async () => {
    vi.mocked(api.getConversationGitStatus).mockResolvedValue({
      kind: 'snapshot',
      checkout_status: {
        kind: 'named_branch',
        branch_name: 'feature',
        head_oid: 'abc123',
        remote_status: { kind: 'tracked', remote_ref: 'origin/feature', ahead: 2, behind: 1 },
      },
      counts: { changed_paths: 0, staged_paths: 0, unstaged_paths: 0, untracked_paths: 0, conflicted_paths: 0 },
      changed_paths: [],
    });
    renderPanel();

    expect(await screen.findByText('feature · origin/feature · ↑2 ↓1')).toBeInTheDocument();
  });

  it('hides Workspace Diff when the conversation mode is not diffable', async () => {
    vi.mocked(api.getConversationGitStatus).mockResolvedValue({
      kind: 'snapshot',
      checkout_status: { kind: 'named_branch', branch_name: 'feature', head_oid: 'abc123', remote_status: { kind: 'no_known' } },
      counts: { changed_paths: 1, staged_paths: 0, unstaged_paths: 1, untracked_paths: 0, conflicted_paths: 0 },
      changed_paths: [],
    });
    renderPanel('conv-1', false);

    expect(await screen.findByText('Changes not staged for commit')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Open Git diff' })).not.toBeInTheDocument();
  });

  it('renders detached live checkout identity without crowding the header', async () => {
    vi.mocked(api.getConversationGitStatus).mockResolvedValue({
      kind: 'snapshot',
      checkout_status: { kind: 'detached', head_oid: 'abcdef1234567890', pointing_refs: [] },
      counts: { changed_paths: 0, staged_paths: 0, unstaged_paths: 0, untracked_paths: 0, conflicted_paths: 0 },
      changed_paths: [],
    });
    renderPanel();

    expect(await screen.findByText('detached @ abcdef1')).toBeInTheDocument();
    expect(screen.getByText('Git').closest('button')).not.toHaveTextContent('detached');
  });

  it('renders the standard clean working tree message', async () => {
    vi.mocked(api.getConversationGitStatus).mockResolvedValue({
      kind: 'snapshot',
      checkout_status: { kind: 'named_branch', branch_name: 'main', head_oid: 'abc123', remote_status: { kind: 'no_known' } },
      counts: { changed_paths: 0, staged_paths: 0, unstaged_paths: 0, untracked_paths: 0, conflicted_paths: 0 },
      changed_paths: [],
    });
    renderPanel();

    expect(await screen.findByText('nothing to commit, working tree clean')).toBeInTheDocument();
    expect(screen.queryByText('Untracked files')).not.toBeInTheDocument();
  });

  it('reloads Git status when the desktop panel expands', async () => {
    vi.mocked(api.getConversationGitStatus).mockResolvedValue({ kind: 'non_git' });
    const view = renderPanel('conv-1', true, true);
    await waitFor(() => expect(api.getConversationGitStatus).toHaveBeenCalledTimes(1));

    view.rerender(
      <MemoryRouter initialEntries={['/c/slug']}>
        <ConversationProvider>
          <ViewerSlotProvider scopeKey="conv-1" browserSessionActive={false}>
            <FileExplorerProvider>
              <FileExplorerPanel
                collapsed={false}
                onToggle={() => {}}
                rootPath="/repo"
                conversationId="conv-1"
                showToast={() => {}}
                showError={() => {}}
                branchName="main"
                activeSlug="slug"
                canOpenWorkspaceDiff
              />
            </FileExplorerProvider>
          </ViewerSlotProvider>
        </ConversationProvider>
      </MemoryRouter>,
    );

    await waitFor(() => expect(api.getConversationGitStatus).toHaveBeenCalledTimes(2));
  });

  it('advances the file-tree refresh signal when Refresh is clicked', async () => {
    renderPanel();
    await waitFor(() => {
      expect(api.getConversationTaskCount).toHaveBeenCalled();
      expect(api.listConversationSkills).toHaveBeenCalled();
    });

    const tree = screen.getByTestId('file-tree');
    const initialRefreshKey = Number(tree.getAttribute('data-refresh-key'));

    fireEvent.click(screen.getByRole('button', { name: 'Refresh file tree' }));

    await waitFor(() => {
      expect(tree).toHaveAttribute('data-refresh-key', String(initialRefreshKey + 1));
    });
  });

  it('keeps Tasks expanded and preserves task group state after Back', async () => {
    renderPanel();

    fireEvent.click(screen.getByRole('button', { name: /Tasks/ }));
    await screen.findByRole('button', { name: (_, el) => el?.classList.contains('tasks-group-header') === true && el.textContent?.includes('blocked') === true });

    fireEvent.click(screen.getByRole('button', { name: (_, el) => el?.classList.contains('tasks-group-header') === true && el.textContent?.includes('ready') === true }));
    expect(screen.queryByText('ready-one')).not.toBeInTheDocument();

    fireEvent.click(screen.getByText('blocked-one'));
    await screen.findByRole('button', { name: /Back/ });
    fireEvent.click(screen.getByRole('button', { name: /Back/ }));

    const tasksHeader = screen.getByRole('button', { name: /Tasks/ });
    expect(tasksHeader).toHaveAttribute('aria-expanded', 'true');
    expect(await screen.findByText('blocked-one')).toBeInTheDocument();
    expect(screen.queryByText('ready-one')).not.toBeInTheDocument();
  });

  it('keeps Skills expanded and preserves skill group state after Back', async () => {
    renderPanel();

    fireEvent.click(screen.getByRole('button', { name: /Skills/ }));
    await screen.findByText('/project-one');

    fireEvent.click(screen.getByRole('button', { name: (_, el) => el?.classList.contains('skill-group-header') === true && el.textContent?.includes('Built-in') === true }));
    expect(screen.queryByText('/builtin-one')).not.toBeInTheDocument();

    fireEvent.click(screen.getByText('/project-one'));
    await screen.findByRole('button', { name: /Back/ });
    fireEvent.click(screen.getByRole('button', { name: /Back/ }));

    const skillsHeader = screen.getByRole('button', { name: /Skills/ });
    expect(skillsHeader).toHaveAttribute('aria-expanded', 'true');
    expect(await screen.findByText('/project-one')).toBeInTheDocument();
    expect(screen.queryByText('/builtin-one')).not.toBeInTheDocument();
  });

  it('resets task and skill panel state when conversation changes', async () => {
    const { rerender } = renderPanel('conv-1');

    fireEvent.click(screen.getByRole('button', { name: /Tasks/ }));
    await screen.findByRole('button', { name: (_, el) => el?.classList.contains('tasks-group-header') === true && el.textContent?.includes('blocked') === true });
    fireEvent.click(screen.getByRole('button', { name: (_, el) => el?.classList.contains('tasks-group-header') === true && el.textContent?.includes('ready') === true }));
    expect(screen.queryByText('ready-one')).not.toBeInTheDocument();

    rerender(
      <MemoryRouter initialEntries={['/c/other']}>
        <ConversationProvider>
          <ViewerSlotProvider scopeKey="conv-2" browserSessionActive={false}>
            <FileExplorerProvider>
              <FileExplorerPanel
                collapsed={false}
                onToggle={() => {}}
                rootPath="/repo"
                conversationId="conv-2"
                showToast={() => {}}
                showError={() => {}}
                branchName="main"
                activeSlug="other"
              />
            </FileExplorerProvider>
          </ViewerSlotProvider>
        </ConversationProvider>
      </MemoryRouter>,
    );

    const tasksHeader = screen.getByRole('button', { name: /Tasks/ });
    expect(tasksHeader).toHaveAttribute('aria-expanded', 'false');

    fireEvent.click(tasksHeader);
    await waitFor(() => expect(api.listConversationTasks).toHaveBeenCalledWith('conv-2', expect.any(AbortSignal)));
    expect(await screen.findByText('ready-one')).toBeInTheDocument();

    const skillsHeader = screen.getByRole('button', { name: /Skills/ });
    expect(skillsHeader).toHaveAttribute('aria-expanded', 'false');
    expect(within(skillsHeader).getByText('Skills')).toBeInTheDocument();
  });
});
