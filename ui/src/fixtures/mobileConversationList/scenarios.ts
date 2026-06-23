import type { Conversation, ConversationState } from '../../api';
import { mobileConversationListScenarioDefinitions } from './types';
import type { MobileConversationListFixtureData, MobileConversationListScenario } from './types';

const now = Date.parse('2026-06-23T12:00:00Z');
const isoAgo = (minutes: number) => new Date(now - minutes * 60_000).toISOString();

function state(type: ConversationState['type']): ConversationState {
  if (type === 'idle') return { type };
  if (type === 'terminal') return { type };
  if (type === 'error') return { type, message: 'Fixture error', error_kind: 'server_error' };
  if (type === 'awaiting_task_approval') return { type, title: 'Approve mobile fixture', priority: 'p2', plan: 'Fixture approval plan.' };
  return { type } as ConversationState;
}

function conv(id: string, slug: string, overrides: Partial<Conversation> = {}): Conversation {
  return {
    id,
    slug,
    model: 'claude-sonnet-4-6',
    cwd: `/Users/scottopell/dev/phoenix-ide/.phoenix/worktrees/${slug}`,
    created_at: isoAgo(900),
    updated_at: isoAgo(30),
    message_count: 18,
    state: state('idle'),
    presentation_mode: 'idle',
    archived: false,
    project_id: 'phoenix',
    project_name: 'phoenix-ide',
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

const cachedPr = {
  number: 375,
  title: 'Redesign mobile conversations list',
  url: 'https://github.com/example/phoenix/pull/375',
  display_state: 'open' as const,
  base: 'main',
  head: 'task-mobile-list',
};

const activeConversations: Conversation[] = [
  conv('standalone-pr', 'polish-pr-feedback-surface', {
    conv_mode_label: 'WORK',
    cached_pr: cachedPr,
    updated_at: isoAgo(3),
    presentation_mode: 'working',
    state: state('awaiting_llm'),
  }),
  conv('chain-latest', 'mobile-chain-latest-work', {
    conv_mode_label: 'WORK',
    cached_pr: cachedPr,
    updated_at: isoAgo(7),
    work_scope_key: 'worktree:/tmp/mobile-chain',
  }),
  conv('chain-root', 'mobile-chain-root-history', {
    continued_in_conv_id: 'chain-latest',
    chain_name: 'mobile conversations redesign',
    presentation_mode: 'done',
    state: state('terminal'),
    updated_at: isoAgo(1440),
    work_scope_key: 'worktree:/tmp/mobile-chain',
  }),
  conv('approval-latest', 'approval-needed-current-work', {
    continued_in_conv_id: null,
    chain_name: 'approval chain stays current',
    conv_mode_label: 'WORK',
    presentation_mode: 'needs_action',
    state: state('awaiting_task_approval'),
    cached_pr: { ...cachedPr, number: 412 },
    updated_at: isoAgo(12),
    work_scope_key: 'worktree:/tmp/approval-chain',
  }),
  conv('approval-root', 'approval-chain-completed-history', {
    continued_in_conv_id: 'approval-latest',
    chain_name: 'approval chain stays current',
    presentation_mode: 'done',
    state: state('terminal'),
    updated_at: isoAgo(480),
    work_scope_key: 'worktree:/tmp/approval-chain',
  }),
  conv('direct', 'direct-mode-no-pr-with-long-path-context', {
    conv_mode_label: 'DIRECT',
    project_name: undefined,
    project_id: null,
    cwd: '/very/long/path/to/phoenix-mobile-fixture',
    updated_at: isoAgo(90),
  }),
];

export const mobileConversationListFixtureData: MobileConversationListFixtureData = {
  conversations: activeConversations,
  archivedConversations: [
    conv('archived', 'archived-mobile-work', {
      archived: true,
      cached_pr: { ...cachedPr, number: 55, display_state: 'merged' },
      updated_at: isoAgo(3000),
      presentation_mode: 'done',
      state: state('terminal'),
    }),
  ],
};

export const mobileConversationListScenarios: MobileConversationListScenario[] = mobileConversationListScenarioDefinitions.map((scenario) => ({
  ...scenario,
}));
