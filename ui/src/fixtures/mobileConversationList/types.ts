import type { Conversation } from '../../api';

export type MobileConversationListTheme = 'dark' | 'light';
export type MobileConversationListScenarioKind = 'active' | 'archived';

export const mobileConversationListScenarioDefinitions = [
  { id: 'active-dark', title: 'Active mobile list / dark', kind: 'active', theme: 'dark' },
  { id: 'active-light', title: 'Active mobile list / light', kind: 'active', theme: 'light' },
  { id: 'archived-dark', title: 'Archived mobile list / dark', kind: 'archived', theme: 'dark' },
] as const satisfies readonly {
  id: string;
  title: string;
  kind: MobileConversationListScenarioKind;
  theme: MobileConversationListTheme;
}[];

export type MobileConversationListScenarioId = (typeof mobileConversationListScenarioDefinitions)[number]['id'];

export interface MobileConversationListScenario {
  id: MobileConversationListScenarioId;
  title: string;
  kind: MobileConversationListScenarioKind;
  theme: MobileConversationListTheme;
}

export interface MobileConversationListFixtureData {
  conversations: Conversation[];
  archivedConversations: Conversation[];
}
