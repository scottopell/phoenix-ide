export const mobileMultiPrConversationScenarioDefinitions = [
  { id: 'collapsed', title: 'Collapsed conversation status', expanded: false, chooserOpen: false },
  { id: 'expanded', title: 'Expanded conversation status', expanded: true, chooserOpen: false },
  { id: 'chooser-open', title: 'In-flight StateBar active-PR dialog', expanded: true, chooserOpen: false, inFlight: true, stateBarDialog: 'pr' },
  { id: 'model-dialog', title: 'Idle StateBar model and effort dialog', expanded: true, chooserOpen: false, stateBarDialog: 'model' },
  { id: 'model-locked', title: 'In-flight StateBar model lock', expanded: true, chooserOpen: false, inFlight: true },
  { id: 'active-pr-actions', title: 'Work Actions for one active PR', expanded: false, chooserOpen: false },
  { id: 'mixed-branch-history', title: 'Open PR with new comments and closed sibling branch', expanded: false, chooserOpen: false },
  { id: 'mixed-branch-work-sheet', title: 'Work sheet with open and closed branch history', expanded: false, chooserOpen: true },
] as const;

export type MobileMultiPrConversationScenarioId =
  (typeof mobileMultiPrConversationScenarioDefinitions)[number]['id'];

export interface MobileMultiPrConversationScenario {
  id: MobileMultiPrConversationScenarioId;
  title: string;
  expanded: boolean;
  chooserOpen: boolean;
  inFlight?: boolean;
  stateBarDialog?: 'pr' | 'model';
}
