import type { Conversation, ConversationState, PrDisplayState } from '../../api';
import { mobileConversationListScenarioDefinitions } from './types';
import type { MobileConversationListFixtureData, MobileConversationListScenario } from './types';

const now = Date.parse('2026-06-23T12:00:00Z');
const isoAgo = (minutes: number) => new Date(now - minutes * 60_000).toISOString();

function state(type: ConversationState['type']): ConversationState {
  switch (type) {
    case 'idle':
    case 'terminal':
    case 'awaiting_llm':
      return { type };
    case 'error':
      return { type, message: 'Fixture error', error_kind: 'server_error' };
    case 'awaiting_task_approval':
      return { type, title: 'Approve mobile fixture', priority: 'p2', plan: 'Fixture approval plan.' };
    case 'awaiting_user_response':
      return { type, questions: [] };
    case 'context_exhausted':
      return { type, summary: 'Context window exhausted' };
    default:
      return { type } as ConversationState;
  }
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

const pr = (number: number, display_state: PrDisplayState = 'open', title = 'Redesign mobile conversations list') => ({
  number,
  title,
  url: `https://github.com/example/phoenix/pull/${number}`,
  display_state,
  base: 'main',
  head: `task-mobile-list-${number}`,
});

const overviewConversations: Conversation[] = [
  conv('state-working-open-pr', 'polish-pr-feedback-surface', {
    conv_mode_label: 'WORK',
    cached_pr: pr(375, 'open'),
    updated_at: isoAgo(3),
    presentation_mode: 'working',
    state: state('awaiting_llm'),
    message_count: 42,
  }),
  conv('state-approval-draft-pr', 'approve-release-note-copy', {
    conv_mode_label: 'WORK',
    cached_pr: pr(412, 'draft', 'Draft release notes'),
    updated_at: isoAgo(9),
    presentation_mode: 'needs_action',
    state: state('awaiting_task_approval'),
  }),
  conv('state-user-question', 'answer-product-question', {
    conv_mode_label: 'EXPLORE',
    updated_at: isoAgo(20),
    presentation_mode: 'needs_action',
    state: state('awaiting_user_response'),
    message_count: 7,
  }),
  conv('state-context-full', 'continue-context-exhausted-investigation', {
    conv_mode_label: 'WORK',
    updated_at: isoAgo(32),
    presentation_mode: 'needs_action',
    state: state('context_exhausted'),
    message_count: 64,
  }),
  conv('state-error-closed-pr', 'fix-failing-mobile-row', {
    conv_mode_label: 'BRANCH',
    cached_pr: pr(391, 'closed', 'Closed design experiment'),
    updated_at: isoAgo(45),
    presentation_mode: 'error',
    state: state('error'),
  }),
  conv('state-ready-no-pr', 'explore-compact-metadata-density', {
    conv_mode_label: 'EXPLORE',
    updated_at: isoAgo(110),
    presentation_mode: 'idle',
    state: state('idle'),
    message_count: 1,
  }),
  conv('state-completed-merged-pr', 'land-mobile-list-styling', {
    conv_mode_label: 'WORK',
    cached_pr: pr(344, 'merged', 'Land mobile list styling'),
    updated_at: isoAgo(280),
    presentation_mode: 'done',
    state: state('terminal'),
  }),
];

const chainConversations: Conversation[] = [
  conv('collapsed-chain-latest', 'handoff-copy-polish', {
    conv_mode_label: 'WORK',
    cached_pr: pr(509, 'open', 'Chain latest PR'),
    updated_at: isoAgo(7),
    work_scope_key: 'worktree:/tmp/mobile-chain',
  }),
  conv('collapsed-chain-root', 'mobile-chain-root-history', {
    continued_in_conv_id: 'collapsed-chain-latest',
    chain_name: 'mobile conversations redesign',
    presentation_mode: 'done',
    state: state('terminal'),
    updated_at: isoAgo(1440),
    work_scope_key: 'worktree:/tmp/mobile-chain',
  }),
  conv('approval-chain-latest', 'approval-needed-current-work', {
    conv_mode_label: 'WORK',
    continued_in_conv_id: null,
    chain_name: 'approval chain stays current',
    presentation_mode: 'needs_action',
    state: state('awaiting_task_approval'),
    cached_pr: pr(412, 'draft', 'Approval chain PR'),
    updated_at: isoAgo(12),
    work_scope_key: 'worktree:/tmp/approval-chain',
  }),
  conv('approval-chain-root', 'approval-chain-completed-history', {
    continued_in_conv_id: 'approval-chain-latest',
    chain_name: 'approval chain stays current',
    presentation_mode: 'done',
    state: state('terminal'),
    updated_at: isoAgo(480),
    work_scope_key: 'worktree:/tmp/approval-chain',
  }),
  conv('cleanup-chain-part-4', 'cleanup-after-merged-pr', {
    conv_mode_label: 'WORK',
    cached_pr: pr(622, 'merged', 'Cleaned-up chain PR'),
    presentation_mode: 'done',
    state: state('terminal'),
    updated_at: isoAgo(90),
    work_scope_key: 'worktree:/tmp/cleanup-chain',
  }),
  conv('cleanup-chain-part-3', 'implement-final-work-changes', {
    continued_in_conv_id: 'cleanup-chain-part-4',
    conv_mode_label: 'WORK',
    presentation_mode: 'done',
    state: state('terminal'),
    updated_at: isoAgo(240),
    work_scope_key: 'worktree:/tmp/cleanup-chain',
  }),
  conv('cleanup-chain-part-2', 'switch-from-explore-to-work', {
    continued_in_conv_id: 'cleanup-chain-part-3',
    conv_mode_label: 'WORK',
    presentation_mode: 'done',
    state: state('terminal'),
    updated_at: isoAgo(420),
    work_scope_key: 'worktree:/tmp/cleanup-chain',
  }),
  conv('cleanup-chain-part-1', 'explore-mobile-row-options', {
    continued_in_conv_id: 'cleanup-chain-part-2',
    chain_name: 'explore → work → cleanup',
    conv_mode_label: 'EXPLORE',
    presentation_mode: 'done',
    state: state('terminal'),
    updated_at: isoAgo(720),
    work_scope_key: 'worktree:/tmp/cleanup-chain',
  }),
];

const namingContextConversations: Conversation[] = [
  conv('task-title-fallback', 'f872dd1a-f701-49f3-ad25-2605c6b6f3dc', {
    conv_mode_label: 'WORK',
    task_title: 'Iterate mobile conversation list fixtures',
    branch_name: 'task-26004-iterate-mobile-conversation-list-fixtures',
    updated_at: isoAgo(6),
    cached_pr: pr(26004, 'open', 'Fixture matrix'),
  }),
  conv('branch-fallback', '9d1b4cc93b7845228e4fdbe566761f44', {
    conv_mode_label: 'BRANCH',
    task_title: null,
    branch_name: 'scott/mobile-row-overflow-audit',
    updated_at: isoAgo(16),
    project_name: null,
    cwd: '/Users/scottopell/dev/phoenix-ide',
  }),
  conv('cwd-leaf-fallback', '123e4567-e89b-12d3-a456-426614174000', {
    conv_mode_label: 'DIRECT',
    task_title: null,
    branch_name: null,
    project_name: null,
    project_id: null,
    cwd: '/very/long/path/to/phoenix-mobile-fixture-with-readable-leaf',
    updated_at: isoAgo(60),
  }),
  conv('long-title', 'investigate-mobile-conversation-list-layout-with-an-exceptionally-long-human-readable-title-that-must-truncate', {
    conv_mode_label: 'EXPLORE',
    updated_at: isoAgo(120),
    message_count: 128,
  }),
  conv('missing-mode', 'conversation-without-mode-label', {
    conv_mode_label: '',
    project_name: 'phoenix-ide',
    updated_at: isoAgo(180),
  }),
  conv('unknown-mode', 'unknown-managed-mode-label', {
    conv_mode_label: 'CUSTOM',
    project_name: null,
    cwd: '/tmp/custom-mode-context',
    updated_at: isoAgo(240),
  }),
];

const archivedConversations: Conversation[] = [
  conv('archived-merged', 'archived-mobile-work', {
    archived: true,
    cached_pr: pr(55, 'merged', 'Archived merged work'),
    updated_at: isoAgo(3000),
    presentation_mode: 'done',
    state: state('terminal'),
  }),
  conv('archived-closed', 'archived-closed-pr-cleanup', {
    archived: true,
    conv_mode_label: 'BRANCH',
    cached_pr: pr(56, 'closed', 'Archived closed branch'),
    updated_at: isoAgo(4400),
    presentation_mode: 'error',
    state: state('error'),
  }),
  conv('archived-guid-fallback', 'a81bc81b-dead-4f56-a0c5-8f1c071983ad', {
    archived: true,
    task_title: 'Archived work still has a readable title',
    updated_at: isoAgo(9000),
    presentation_mode: 'done',
    state: state('terminal'),
  }),
];

const fixtureDataByDataset: Record<MobileConversationListScenario['dataset'], MobileConversationListFixtureData> = {
  overview: {
    conversations: overviewConversations,
    archivedConversations,
  },
  chains: {
    conversations: chainConversations,
    archivedConversations,
  },
  'naming-context': {
    conversations: namingContextConversations,
    archivedConversations,
  },
  archived: {
    conversations: overviewConversations,
    archivedConversations,
  },
};

export const mobileConversationListScenarios: MobileConversationListScenario[] = mobileConversationListScenarioDefinitions.map((scenario) => ({
  ...scenario,
}));

export function getMobileConversationListScenario(id: string | null | undefined): MobileConversationListScenario {
  return mobileConversationListScenarios.find((scenario) => scenario.id === id)
    ?? mobileConversationListScenarios[0]!;
}

export function getMobileConversationListFixtureData(scenario: MobileConversationListScenario): MobileConversationListFixtureData {
  return fixtureDataByDataset[scenario.dataset];
}
