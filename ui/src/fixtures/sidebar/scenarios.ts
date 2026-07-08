import type { Conversation, ConversationState, Project, PrDisplayState } from '../../api';
import type { SidebarFixtureData, SidebarScenario } from './types';

const now = Date.parse('2026-07-08T16:00:00Z');
const isoAgo = (minutes: number) => new Date(now - minutes * 60_000).toISOString();

function state(type: ConversationState['type']): ConversationState {
  switch (type) {
    case 'error':
      return { type, message: 'Fixture error', error_kind: 'server_error' };
    case 'awaiting_task_approval':
      return { type, title: 'Approve sidebar polish', priority: 'p2', plan: 'Verify the sidebar fixture states.' };
    case 'awaiting_user_response':
      return { type, questions: [] };
    case 'context_exhausted':
      return { type, summary: 'Context window exhausted' };
    default:
      return { type } as ConversationState;
  }
}

const projects: Project[] = [
  { id: 'phoenix', canonical_path: '/Users/scottopell/dev/phoenix-ide', main_ref: 'main', created_at: isoAgo(8000), conversation_count: 0 },
  { id: 'agents', canonical_path: '/Users/scottopell/dev/agent-platform', main_ref: 'main', created_at: isoAgo(7000), conversation_count: 0 },
  { id: 'docs', canonical_path: '/Users/scottopell/dev/docs-playground', main_ref: 'main', created_at: isoAgo(6000), conversation_count: 0 },
];

const pr = (number: number, display_state: PrDisplayState = 'open') => ({
  number,
  title: 'Sidebar fixture PR',
  url: `https://github.com/example/phoenix/pull/${number}`,
  display_state,
  base: 'main',
  head: `task-sidebar-${number}`,
});

function conv(id: string, slug: string, projectId: string, overrides: Partial<Conversation> = {}): Conversation {
  const project = projects.find((p) => p.id === projectId) ?? projects[0]!;
  return {
    id,
    slug,
    model: 'claude-sonnet-4-6',
    cwd: `${project.canonical_path}/${slug}`,
    created_at: isoAgo(1200),
    updated_at: isoAgo(45),
    message_count: 18,
    state: state('idle'),
    presentation_mode: 'idle',
    archived: false,
    project_id: projectId,
    project_name: project.canonical_path.split('/').pop() ?? project.canonical_path,
    conv_mode_label: 'EXPLORE',
    branch_name: null,
    worktree_path: null,
    base_branch: null,
    task_title: null,
    parent_conversation_id: null,
    parent_conversation_slug: null,
    user_initiated: true,
    seed_parent_id: null,
    seed_label: null,
    seed_parent_slug: null,
    continued_in_conv_id: null,
    chain_name: null,
    browser_session_active: false,
    terminal_uses_tmux: true,
    work_scope_key: `conversation:${id}`,
    ...overrides,
  };
}

const conversations: Conversation[] = [
  conv('phoenix-working', 'sidebar-lifecycle-polish', 'phoenix', {
    conv_mode_label: 'WORK',
    presentation_mode: 'working',
    state: state('awaiting_llm'),
    updated_at: isoAgo(2),
    cached_pr: pr(47001, 'open'),
    message_count: 54,
  }),
  conv('phoenix-approval', 'approve-project-count-copy', 'phoenix', {
    conv_mode_label: 'WORK',
    presentation_mode: 'needs_action',
    state: state('awaiting_task_approval'),
    updated_at: isoAgo(8),
    message_count: 22,
  }),
  conv('phoenix-question', 'answer-sidebar-product-question', 'phoenix', {
    presentation_mode: 'needs_action',
    state: state('awaiting_user_response'),
    updated_at: isoAgo(15),
  }),
  conv('phoenix-error', 'fix-sidebar-empty-state-regression', 'phoenix', {
    conv_mode_label: 'BRANCH',
    presentation_mode: 'error',
    state: state('error'),
    updated_at: isoAgo(28),
    cached_pr: pr(47002, 'closed'),
  }),
  conv('phoenix-done', 'land-collapsed-dot-overflow', 'phoenix', {
    conv_mode_label: 'WORK',
    presentation_mode: 'done',
    state: state('terminal'),
    updated_at: isoAgo(80),
    cached_pr: pr(47000, 'merged'),
  }),
  conv('agents-working', 'agent-platform-sidebar-review', 'agents', {
    conv_mode_label: 'WORK',
    presentation_mode: 'working',
    state: state('awaiting_llm'),
    updated_at: isoAgo(4),
  }),
  conv('agents-ready', 'explore-agent-navigation-density', 'agents', {
    updated_at: isoAgo(35),
  }),
  conv('agents-done', 'ship-agent-filter-copy', 'agents', {
    presentation_mode: 'done',
    state: state('terminal'),
    updated_at: isoAgo(180),
  }),
  ...Array.from({ length: 8 }, (_, index) => conv(`overflow-${index}`, `collapsed-overflow-${index + 1}`, index % 2 === 0 ? 'phoenix' : 'agents', {
    updated_at: isoAgo(220 + index * 20),
    presentation_mode: index === 5 ? 'needs_action' : 'idle',
    state: index === 5 ? state('awaiting_user_response') : state('idle'),
  })),
];

const archivedConversations: Conversation[] = [
  conv('phoenix-archived-1', 'archived-sidebar-discovery', 'phoenix', {
    archived: true,
    presentation_mode: 'done',
    state: state('terminal'),
    updated_at: isoAgo(300),
  }),
  conv('phoenix-archived-2', 'archived-project-scope-prototype', 'phoenix', {
    archived: true,
    presentation_mode: 'done',
    state: state('terminal'),
    updated_at: isoAgo(520),
  }),
  conv('agents-archived-1', 'archived-agent-nav-experiment', 'agents', {
    archived: true,
    presentation_mode: 'done',
    state: state('terminal'),
    updated_at: isoAgo(640),
  }),
];

export const sidebarFixtureData: SidebarFixtureData = {
  projects,
  conversations,
  archivedConversations,
};

export const sidebarScenarios: SidebarScenario[] = [
  {
    id: 'expanded-all-active',
    theme: 'dark',
    collapsed: false,
    initialProjectId: null,
    activeSlug: 'sidebar-lifecycle-polish',
  },
  {
    id: 'expanded-project-archived',
    theme: 'dark',
    collapsed: false,
    initialProjectId: 'phoenix',
    activeSlug: 'archived-sidebar-discovery',
  },
  {
    id: 'expanded-empty-project',
    theme: 'light',
    collapsed: false,
    initialProjectId: 'docs',
    activeSlug: null,
  },
  {
    id: 'collapsed-overflow',
    theme: 'dark',
    collapsed: true,
    initialProjectId: null,
    activeSlug: 'collapsed-overflow-8',
  },
];
