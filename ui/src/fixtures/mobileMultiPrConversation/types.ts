export const mobileMultiPrConversationScenarioDefinitions = [
  { id: 'collapsed', title: 'Collapsed conversation status', expanded: false, chooserOpen: false },
  { id: 'expanded', title: 'Expanded conversation status', expanded: true, chooserOpen: false },
  { id: 'chooser-open', title: 'Two open PR choices', expanded: true, chooserOpen: true },
  { id: 'active-pr-actions', title: 'Work Actions for one active PR', expanded: false, chooserOpen: false },
  { id: 'mixed-branch-history', title: 'Open PR with new comments and closed sibling branch', expanded: false, chooserOpen: false },
] as const;

export type MobileMultiPrConversationScenarioId =
  (typeof mobileMultiPrConversationScenarioDefinitions)[number]['id'];

export interface MobileMultiPrConversationScenario {
  id: MobileMultiPrConversationScenarioId;
  title: string;
  expanded: boolean;
  chooserOpen: boolean;
}
