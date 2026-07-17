import type {
  AssociatedPrStatusEnvelope,
  AssociatedPrSummaryResponse,
  Conversation,
  PrStatusResponse,
} from '../../api';
import { mobileMultiPrConversationScenarioDefinitions } from './types';
import type { MobileMultiPrConversationScenario } from './types';

export const mobileMultiPrConversationScenarios: MobileMultiPrConversationScenario[] =
  mobileMultiPrConversationScenarioDefinitions.map((scenario) => ({ ...scenario }));

export function getMobileMultiPrConversationScenario(id: string): MobileMultiPrConversationScenario {
  const scenario = mobileMultiPrConversationScenarios.find((item) => item.id === id);
  if (!scenario) throw new Error(`Unknown mobile multi-PR conversation scenario: ${id}`);
  return scenario;
}

export const mobileMultiPrConversation: Conversation = {
  id: 'fixture-mobile-multi-pr',
  slug: 'polish-mobile-multi-pr-conversation-ui',
  model: 'claude-sonnet-4-6',
  cwd: '/Users/dev/phoenix-ide/.phoenix/worktrees/mobile-multi-pr-ui',
  created_at: '2026-07-15T14:00:00Z',
  updated_at: '2026-07-15T14:30:00Z',
  message_count: 28,
  state: { type: 'idle' },
  presentation_mode: 'idle',
  branch_name: 'task-58045-mobile-multi-pr-conversation-fixture',
  worktree_path: '/Users/dev/phoenix-ide/.phoenix/worktrees/mobile-multi-pr-ui',
  base_branch: 'main',
  task_title: 'Improve conversations with multiple pull requests on mobile',
  archived: false,
  project_id: 'phoenix-fixture',
  project_name: 'phoenix-ide',
  conv_mode_label: 'Work',
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
  work_scope_key: 'worktree:/Users/dev/phoenix-ide/.phoenix/worktrees/mobile-multi-pr-ui',
};

export const mobileMultiPrAssociatedPrs: AssociatedPrSummaryResponse[] = [
  {
    repo_owner: 'phoenix-ide',
    repo_name: 'phoenix-ide',
    pr_number: 417,
    title: 'Add durable multi-PR conversation association',
    url: 'https://github.com/phoenix-ide/phoenix-ide/pull/417',
    state: 'OPEN',
    draft: false,
    display_state: 'open',
    base: 'main',
    head: 'feature/multi-pr-association',
    github_updated_at: '2026-07-15T14:24:00Z',
    feedback_status: 'in_progress',
  },
  {
    repo_owner: 'phoenix-ide',
    repo_name: 'phoenix-ide',
    pr_number: 423,
    title: 'Follow up with mobile active-PR selection',
    url: 'https://github.com/phoenix-ide/phoenix-ide/pull/423',
    state: 'OPEN',
    draft: false,
    display_state: 'open',
    base: 'feature/multi-pr-association',
    head: 'feature/mobile-pr-selector',
    github_updated_at: '2026-07-15T14:29:00Z',
    feedback_status: 'approved',
  },
];

export const mobileMultiPrSelection: AssociatedPrStatusEnvelope = {
  associated_prs: mobileMultiPrAssociatedPrs,
  latest_observed_branch: {
    repository_identity: 'phoenix-ide/phoenix-ide',
    branch_name: 'task-58045-mobile-multi-pr-conversation-fixture',
  },
};

export const mobileMultiPrStatus: PrStatusResponse = {
  found: false,
  refresh: {
    state: 'fresh',
    last_attempted_at: '2026-07-15T14:30:00Z',
    last_refreshed_at: '2026-07-15T14:30:00Z',
    stale: false,
  },
  work_change: { kind: 'clean' },
  selection: mobileMultiPrSelection,
};

export const mobileMultiPrActiveSelection: AssociatedPrStatusEnvelope = {
  ...mobileMultiPrSelection,
  active_pr: {
    pr: { repo_owner: 'phoenix-ide', repo_name: 'phoenix-ide', pr_number: 423 },
    provenance: 'pinned',
  },
};

export const mobileMultiPrActiveStatus: PrStatusResponse = {
  found: true,
  number: 423,
  title: mobileMultiPrAssociatedPrs[1]!.title,
  url: mobileMultiPrAssociatedPrs[1]!.url,
  state: 'OPEN',
  draft: false,
  base: mobileMultiPrAssociatedPrs[1]!.base,
  head: mobileMultiPrAssociatedPrs[1]!.head,
  display_state: 'open',
  check_state: 'failing',
  feedback_freshness: { state: 'new', count: 2 },
  feedback_status: 'open',
  refresh: {
    state: 'fresh',
    last_attempted_at: '2026-07-15T14:30:00Z',
    last_refreshed_at: '2026-07-15T14:30:00Z',
    stale: false,
  },
  work_change: { kind: 'clean' },
  selection: mobileMultiPrActiveSelection,
};

export const mobileMixedBranchPrs: AssociatedPrSummaryResponse[] = [
  {
    repo_owner: 'phoenix-ide',
    repo_name: 'phoenix-ide',
    pr_number: 417,
    title: 'Add durable multi-PR conversation association',
    url: 'https://github.com/phoenix-ide/phoenix-ide/pull/417',
    state: 'CLOSED',
    draft: false,
    display_state: 'closed',
    base: 'main',
    head: 'feature/multi-pr-association',
    github_updated_at: '2026-07-15T14:24:00Z',
    feedback_status: 'open',
  },
  {
    repo_owner: 'phoenix-ide',
    repo_name: 'phoenix-ide',
    pr_number: 423,
    title: 'Follow up with mobile active-PR selection',
    url: 'https://github.com/phoenix-ide/phoenix-ide/pull/423',
    state: 'OPEN',
    draft: false,
    display_state: 'open',
    base: 'main',
    head: 'feature/mobile-pr-selector',
    github_updated_at: '2026-07-15T14:29:00Z',
    feedback_status: 'open',
  },
];

export const mobileMixedBranchSelection: AssociatedPrStatusEnvelope = {
  associated_prs: mobileMixedBranchPrs,
  active_pr: {
    pr: { repo_owner: 'phoenix-ide', repo_name: 'phoenix-ide', pr_number: 423 },
    provenance: 'inferred',
  },
  latest_observed_branch: {
    repository_identity: 'phoenix-ide/phoenix-ide',
    branch_name: 'feature/mobile-pr-selector',
  },
};

export const mobileMixedBranchStatus: PrStatusResponse = {
  ...mobileMultiPrActiveStatus,
  check_state: 'passing',
  feedback_freshness: { state: 'new', count: 3 },
  selection: mobileMixedBranchSelection,
};
