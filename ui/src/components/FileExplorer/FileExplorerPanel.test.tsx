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

function panelTree(conversationId = 'conv-1', instructionSnapshotVersion = 1) {
  return (
    <MemoryRouter initialEntries={['/c/slug']}>
      <ConversationProvider>
        <ViewerSlotProvider scopeKey={conversationId} browserSessionActive={false}>
          <FileExplorerProvider>
            <FileExplorerPanel
              collapsed={false}
              onToggle={() => {}}
              rootPath="/repo"
              conversationId={conversationId}
              instructionSnapshotVersion={instructionSnapshotVersion}
              showToast={() => {}}
              showError={() => {}}
              branchName="main"
              activeSlug="slug"
            />
          </FileExplorerProvider>
        </ViewerSlotProvider>
      </ConversationProvider>
    </MemoryRouter>
  );
}

function renderPanel(conversationId = 'conv-1', instructionSnapshotVersion = 1) {
  return render(panelTree(conversationId, instructionSnapshotVersion));
}

beforeEach(() => {
  vi.mocked(api.getConversationTaskCount).mockResolvedValue({ active: 2, closed: 1, blocked: 1, current: false });
  vi.mocked(api.listConversationTasks).mockResolvedValue({ tasks });
  vi.mocked(api.listConversationSkills).mockResolvedValue({ skills });
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

  it('renders the captured conversation skill body without reading the live file', async () => {
    const capturedSkills: SkillEntry[] = [{
      ...skill('project-one', 'project', '/repo/project/.agents/skills/project-one/SKILL.md'),
      body: 'old captured body',
    }];
    vi.mocked(api.listConversationSkills).mockResolvedValue({ skills: capturedSkills });

    renderPanel();
    fireEvent.click(screen.getByRole('button', { name: /Skills/ }));
    fireEvent.click(await screen.findByText('/project-one'));

    expect(await screen.findByText('old captured body')).toBeInTheDocument();
    expect(vi.mocked(fetch).mock.calls.some(([url]) => String(url).startsWith('/api/files/read'))).toBe(false);
  });

  it('refreshes a selected skill body when the same conversation version advances', async () => {
    const oldSkill = { ...skill('project-one', 'project', '/repo/project/.agents/skills/project-one/SKILL.md'), body: 'old captured body' };
    const newSkill = { ...oldSkill, body: 'new captured body' };
    vi.mocked(api.listConversationSkills)
      .mockResolvedValueOnce({ skills: [oldSkill] })
      .mockResolvedValueOnce({ skills: [newSkill] });

    const view = renderPanel('conv-1', 1);
    fireEvent.click(screen.getByRole('button', { name: /Skills/ }));
    fireEvent.click(await screen.findByText('/project-one'));
    expect(await screen.findByText('old captured body')).toBeInTheDocument();

    view.rerender(panelTree('conv-1', 2));

    expect(await screen.findByText('new captured body')).toBeInTheDocument();
    expect(screen.queryByText('old captured body')).not.toBeInTheDocument();
    expect(api.listConversationSkills).toHaveBeenCalledTimes(2);
  });

  it('closes the skill viewer when a refresh removes the selected skill', async () => {
    const selected = { ...skill('project-one', 'project', '/repo/project/.agents/skills/project-one/SKILL.md'), body: 'captured body' };
    vi.mocked(api.listConversationSkills)
      .mockResolvedValueOnce({ skills: [selected] })
      .mockResolvedValueOnce({ skills: [] });

    const view = renderPanel('conv-1', 1);
    fireEvent.click(screen.getByRole('button', { name: /Skills/ }));
    fireEvent.click(await screen.findByText('/project-one'));
    expect(await screen.findByText('captured body')).toBeInTheDocument();

    view.rerender(panelTree('conv-1', 2));

    expect(await screen.findByText('No skills discovered for this conversation.')).toBeInTheDocument();
    expect(screen.queryByText('captured body')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Back/ })).not.toBeInTheDocument();
  });

  it('does not refetch skills for an ordinary rerender with an unchanged version', async () => {
    const view = renderPanel('conv-1', 1);
    await waitFor(() => expect(api.listConversationSkills).toHaveBeenCalledTimes(1));

    view.rerender(panelTree('conv-1', 1));

    await waitFor(() => expect(api.listConversationSkills).toHaveBeenCalledTimes(1));
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
