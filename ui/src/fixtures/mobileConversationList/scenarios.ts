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

const longStandaloneConversations: Conversation[] = Array.from({ length: 18 }, (_, index) => {
  const n = index + 1;
  const mode = n % 4 === 0 ? 'WORK' : n % 4 === 1 ? 'EXPLORE' : n % 4 === 2 ? 'BRANCH' : 'DIRECT';
  const stateType: ConversationState['type'] = n % 6 === 0
    ? 'awaiting_llm'
    : n % 5 === 0
      ? 'error'
      : n % 4 === 0
        ? 'awaiting_user_response'
        : 'idle';
  const presentationMode = stateType === 'awaiting_llm'
    ? 'working'
    : stateType === 'error'
      ? 'error'
      : stateType === 'awaiting_user_response'
        ? 'needs_action'
        : 'idle';

  return conv(`long-standalone-${n}`, `long-standalone-${n.toString().padStart(2, '0')}`, {
    conv_mode_label: mode,
    updated_at: isoAgo(14 + n * 11),
    created_at: isoAgo(1400 + n * 37),
    message_count: 2 + n * 3,
    state: state(stateType),
    presentation_mode: presentationMode,
    task_title: n % 3 === 0 ? `Standalone mobile fixture row ${n}` : null,
    branch_name: n % 4 === 2 ? `scott/mobile-long-row-${n}` : null,
    project_name: n % 5 === 0 ? null : 'phoenix-ide',
    project_id: n % 5 === 0 ? null : 'phoenix',
    cwd: n % 5 === 0 ? `/tmp/mobile-long-row-${n}` : `/Users/scottopell/dev/phoenix-ide/.phoenix/worktrees/long-standalone-${n.toString().padStart(2, '0')}`,
    cached_pr: n % 6 === 1
      ? pr(800 + n, 'open', `Long list open PR ${n}`)
      : n % 6 === 3
        ? pr(800 + n, 'draft', `Long list draft PR ${n}`)
        : n % 6 === 5
          ? pr(800 + n, 'closed', `Long list closed PR ${n}`)
          : null,
  });
});

const longContinuationChain: Conversation[] = [
  conv('long-chain-current-12', 'long-chain-current-12', {
    conv_mode_label: 'WORK',
    chain_name: 'mobile long continuation chain',
    cached_pr: pr(912, 'open', 'Current long chain PR'),
    updated_at: isoAgo(2),
    created_at: isoAgo(55),
    message_count: 87,
    state: state('awaiting_llm'),
    presentation_mode: 'working',
    task_title: 'Rework mobile conversation list scrolling',
    branch_name: 'task-24704-rework-mobile-conversation-list',
    work_scope_key: 'worktree:/tmp/mobile-long-chain',
  }),
  conv('long-chain-link-11', 'long-chain-link-11', {
    continued_in_conv_id: 'long-chain-current-12',
    conv_mode_label: 'WORK',
    updated_at: isoAgo(18),
    created_at: isoAgo(130),
    message_count: 63,
    state: state('awaiting_task_approval'),
    presentation_mode: 'needs_action',
    cached_pr: pr(911, 'draft', 'Approval step before current chain'),
    task_title: 'Queue the follow-up continuation',
    work_scope_key: 'worktree:/tmp/mobile-long-chain',
  }),
  conv('long-chain-link-10', 'long-chain-link-10', {
    continued_in_conv_id: 'long-chain-link-11',
    conv_mode_label: 'EXPLORE',
    updated_at: isoAgo(36),
    created_at: isoAgo(210),
    message_count: 21,
    state: state('awaiting_user_response'),
    presentation_mode: 'needs_action',
    task_title: 'Clarify mobile chain interactions',
    work_scope_key: 'worktree:/tmp/mobile-long-chain',
  }),
  conv('long-chain-link-09', 'long-chain-link-09', {
    continued_in_conv_id: 'long-chain-link-10',
    conv_mode_label: 'WORK',
    updated_at: isoAgo(58),
    created_at: isoAgo(340),
    message_count: 54,
    state: state('terminal'),
    presentation_mode: 'done',
    cached_pr: pr(910, 'merged', 'Merged checkpoint for long chain'),
    task_title: 'Land the compact chain summary experiment',
    work_scope_key: 'worktree:/tmp/mobile-long-chain',
  }),
  conv('long-chain-link-08', 'long-chain-link-08', {
    continued_in_conv_id: 'long-chain-link-09',
    conv_mode_label: 'BRANCH',
    updated_at: isoAgo(77),
    created_at: isoAgo(470),
    message_count: 33,
    state: state('error'),
    presentation_mode: 'error',
    branch_name: 'scott/mobile-chain-hotfix',
    task_title: 'Recover from chain layout regression',
    work_scope_key: 'worktree:/tmp/mobile-long-chain',
  }),
  conv('long-chain-link-07', 'long-chain-link-07', {
    continued_in_conv_id: 'long-chain-link-08',
    conv_mode_label: 'WORK',
    updated_at: isoAgo(96),
    created_at: isoAgo(590),
    message_count: 47,
    state: state('context_exhausted'),
    presentation_mode: 'needs_action',
    task_title: 'Continue after context fill-up',
    work_scope_key: 'worktree:/tmp/mobile-long-chain',
  }),
  conv('long-chain-link-06', 'long-chain-link-06', {
    continued_in_conv_id: 'long-chain-link-07',
    conv_mode_label: 'DIRECT',
    updated_at: isoAgo(124),
    created_at: isoAgo(730),
    message_count: 12,
    state: state('terminal'),
    presentation_mode: 'done',
    project_name: null,
    project_id: null,
    cwd: '/tmp/mobile-chain-direct-step',
    task_title: 'Quick direct validation pass',
    work_scope_key: 'worktree:/tmp/mobile-long-chain',
  }),
  conv('long-chain-link-05', 'long-chain-link-05', {
    continued_in_conv_id: 'long-chain-link-06',
    conv_mode_label: 'EXPLORE',
    updated_at: isoAgo(150),
    created_at: isoAgo(860),
    message_count: 18,
    state: state('terminal'),
    presentation_mode: 'done',
    task_title: 'Compare competing mobile row hierarchies',
    work_scope_key: 'worktree:/tmp/mobile-long-chain',
  }),
  conv('long-chain-link-04', 'long-chain-link-04', {
    continued_in_conv_id: 'long-chain-link-05',
    conv_mode_label: 'WORK',
    updated_at: isoAgo(182),
    created_at: isoAgo(1020),
    message_count: 72,
    state: state('terminal'),
    presentation_mode: 'done',
    cached_pr: pr(904, 'closed', 'Closed chain exploration PR'),
    task_title: 'Close the detour branch cleanly',
    work_scope_key: 'worktree:/tmp/mobile-long-chain',
  }),
  conv('long-chain-link-03', 'long-chain-link-03', {
    continued_in_conv_id: 'long-chain-link-04',
    conv_mode_label: 'WORK',
    updated_at: isoAgo(220),
    created_at: isoAgo(1200),
    message_count: 39,
    state: state('terminal'),
    presentation_mode: 'done',
    task_title: 'Prototype the first continuation compression pass',
    work_scope_key: 'worktree:/tmp/mobile-long-chain',
  }),
  conv('long-chain-link-02', 'long-chain-link-02', {
    continued_in_conv_id: 'long-chain-link-03',
    conv_mode_label: 'EXPLORE',
    updated_at: isoAgo(270),
    created_at: isoAgo(1420),
    message_count: 9,
    state: state('terminal'),
    presentation_mode: 'done',
    task_title: 'Inventory long mobile list edge cases',
    work_scope_key: 'worktree:/tmp/mobile-long-chain',
  }),
  conv('long-chain-root-01', 'long-chain-root-01', {
    continued_in_conv_id: 'long-chain-link-02',
    conv_mode_label: 'EXPLORE',
    chain_name: 'mobile long continuation chain',
    updated_at: isoAgo(360),
    created_at: isoAgo(1680),
    message_count: 4,
    state: state('terminal'),
    presentation_mode: 'done',
    task_title: 'Original mobile conversation list bug hunt',
    work_scope_key: 'worktree:/tmp/mobile-long-chain',
  }),
];

const longListConversations: Conversation[] = [
  ...longStandaloneConversations.slice(0, 9),
  ...longContinuationChain,
  ...longStandaloneConversations.slice(9),
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
  'long-list': {
    conversations: longListConversations,
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
