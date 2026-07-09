import type { ConversationPrStatusState } from '../../hooks/useConversationPrStatus';

export const workActionsScenarioDefinitions = [
  { id: 'cached-open-stable', title: 'Cached open PR keeps Address feedback primary' },
  { id: 'fresh-address-feedback', title: 'Fresh open PR with new feedback' },
  { id: 'passing-address-feedback-merge-secondary', title: 'Passing PR with Merge secondary' },
  { id: 'merged-clean-up', title: 'Merged PR clean up' },
  { id: 'no-pr-dirty-review', title: 'No PR dirty work requires review' },
  { id: 'no-pr-create-pr', title: 'No PR dirty pushed work can create PR' },
  { id: 'gh-unavailable', title: 'GitHub unavailable manual cleanup' },
  { id: 'stuck-open-pr', title: 'Stuck conversation suppresses Resolve' },
] as const;

export type WorkActionsScenarioId = (typeof workActionsScenarioDefinitions)[number]['id'];

export interface WorkActionsScenario {
  id: WorkActionsScenarioId;
  title: string;
  description: string;
  convModeLabel: string;
  phaseType: string;
  continuedInConvId: string | null;
  prState: ConversationPrStatusState;
}
