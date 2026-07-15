export const mobileMultiPrConversationScenarioDefinitions = [
  { id: 'collapsed', title: 'Collapsed conversation status', expanded: false, chooserOpen: false },
  { id: 'expanded', title: 'Expanded conversation status', expanded: true, chooserOpen: false },
  { id: 'chooser-open', title: 'Two open PR choices', expanded: true, chooserOpen: true },
] as const;

export type MobileMultiPrConversationScenarioId =
  (typeof mobileMultiPrConversationScenarioDefinitions)[number]['id'];

export interface MobileMultiPrConversationScenario {
  id: MobileMultiPrConversationScenarioId;
  title: string;
  expanded: boolean;
  chooserOpen: boolean;
}
