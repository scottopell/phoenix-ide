import type { Conversation, ConversationState } from '../../api';
import { conversationPanelScenarioDefinitions } from './types';
import type { ConversationPanelFixtureData, ConversationPanelScenario } from './types';

const now = Date.parse('2026-06-22T14:30:00Z');
const isoAgo = (minutes: number) => new Date(now - minutes * 60_000).toISOString();

function state(type: ConversationState['type']): ConversationState {
  if (type === 'idle') return { type };
  if (type === 'error') return { type, message: 'Fixture error', error_kind: 'server_error' };
  if (type === 'terminal') return { type };
  if (type === 'context_exhausted') return { type, summary: 'Fixture summary hit context limit.' };
  if (type === 'awaiting_task_approval') return { type, title: 'Approve seeded task', priority: 'p2', plan: 'Seeded plan for sidebar QA.' };
  if (type === 'awaiting_user_response') return { type, questions: [] };
  return { type } as ConversationState;
}

function conv(id: string, slug: string, overrides: Partial<Conversation> = {}): Conversation {
  return {
    id,
    slug,
    model: 'claude-sonnet-4-6',
    cwd: `/home/dev/phoenix/${slug}`,
    created_at: isoAgo(720),
    updated_at: isoAgo(30),
    message_count: 8,
    state: state('idle'),
    presentation_mode: 'idle',
    branch_name: null,
    worktree_path: null,
    base_branch: null,
    task_title: null,
    archived: false,
    project_id: 'phoenix-fixture',
    project_name: 'phoenix-ide',
    conv_mode_label: 'Explore',
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

const workScope = 'worktree:/home/dev/phoenix/.phoenix/worktrees/task-35396-sidebar-cached-pr-badges';
const cachedOpen = {
  number: 123,
  title: 'Show cached PR badges in the sidebar with a title long enough to exercise tooltip-only overflow',
  url: 'https://github.com/example/phoenix/pull/123',
  display_state: 'open' as const,
  base: 'main',
  head: 'task-35396-sidebar-cached-pr-badges',
};

export const conversationPanelFixtureData: ConversationPanelFixtureData = {
  activeSlug: 'sidebar-cached-pr-badges',
  conversations: [
    conv('work-open', 'sidebar-cached-pr-badges', {
      conv_mode_label: 'Work',
      branch_name: 'task-35396-sidebar-cached-pr-badges',
      worktree_path: '/home/dev/phoenix/.phoenix/worktrees/task-35396-sidebar-cached-pr-badges',
      base_branch: 'main',
      task_title: 'Add cached PR badges to conversation sidebar',
      work_scope_key: workScope,
      cached_pr: cachedOpen,
      presentation_mode: 'working',
      state: state('awaiting_llm'),
      message_count: 42,
      updated_at: isoAgo(2),
      browser_session_active: true,
    }),
    conv('work-draft', 'draft-pr-continuation', {
      conv_mode_label: 'Work',
      work_scope_key: workScope,
      cached_pr: { ...cachedOpen, number: 124, display_state: 'draft', title: 'Draft PR shared work scope', url: 'https://github.com/example/phoenix/pull/124' },
      parent_conversation_id: 'work-open',
      presentation_mode: 'needs_action',
      state: state('context_exhausted'),
      message_count: 17,
      updated_at: isoAgo(8),
    }),
    conv('branch-merged', 'branch-mode-known-merged-pr', {
      conv_mode_label: 'Branch',
      branch_name: 'review/badges',
      base_branch: 'main',
      work_scope_key: 'worktree:/home/dev/phoenix/review-badges',
      cached_pr: { ...cachedOpen, number: 98, display_state: 'merged', title: 'Merged branch conversation', url: 'https://github.com/example/phoenix/pull/98', head: 'review/badges' },
      presentation_mode: 'done',
      state: state('terminal'),
      message_count: 31,
      updated_at: isoAgo(60),
    }),
    conv('branch-closed', 'closed-pr-needs-cleanup', {
      conv_mode_label: 'Branch',
      branch_name: 'stale/closed-pr',
      work_scope_key: 'worktree:/home/dev/phoenix/closed-pr',
      cached_pr: { ...cachedOpen, number: 77, display_state: 'closed', title: 'Closed without merge', url: 'https://github.com/example/phoenix/pull/77', head: 'stale/closed-pr' },
      presentation_mode: 'error',
      state: state('error'),
      message_count: 12,
      updated_at: isoAgo(120),
    }),
    conv('approval', 'awaiting-task-approval-long-name-for-sidebar-wrapping', {
      conv_mode_label: 'Explore',
      presentation_mode: 'needs_action',
      state: state('awaiting_task_approval'),
      message_count: 5,
      updated_at: isoAgo(180),
    }),
    conv('direct', 'direct-mode-no-pr', {
      conv_mode_label: 'Direct',
      presentation_mode: 'idle',
      state: state('idle'),
      message_count: 3,
      updated_at: isoAgo(240),
      project_id: null,
    }),
  ],
  archivedConversations: [
    conv('archived-merged', 'archived-known-merged-pr', {
      archived: true,
      conv_mode_label: 'Work',
      work_scope_key: 'worktree:/home/dev/phoenix/archived-merged',
      cached_pr: { ...cachedOpen, number: 55, display_state: 'merged', title: 'Archived merged PR', url: 'https://github.com/example/phoenix/pull/55', head: 'archived/merged' },
      presentation_mode: 'done',
      state: state('terminal'),
      updated_at: isoAgo(1440),
    }),
  ],
};

export const conversationPanelScenarios: ConversationPanelScenario[] = conversationPanelScenarioDefinitions.map((scenario) => ({
  ...scenario,
}));

export function getConversationPanelScenario(id: string | null | undefined): ConversationPanelScenario {
  return conversationPanelScenarios.find((scenario) => scenario.id === id)
    ?? conversationPanelScenarios.find((scenario) => scenario.kind === id)
    ?? conversationPanelScenarios[0]!;
}
