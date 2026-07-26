import type { Conversation } from '../../api';

export type MobileConversationListTheme = 'dark' | 'light';
export type MobileConversationListScenarioKind = 'active' | 'archived';
export type MobileConversationListFixtureDataset = 'overview' | 'chains' | 'naming-context' | 'archived' | 'long-list';

export const mobileConversationListScenarioDefinitions = [
  { id: 'active-overview-dark', title: 'Active mobile list / state and PR coverage / dark', kind: 'active', theme: 'dark', dataset: 'overview', activeSlug: null },
  { id: 'active-overview-light', title: 'Active mobile list / state and PR coverage / light', kind: 'active', theme: 'light', dataset: 'overview', activeSlug: null },
  { id: 'chains-dark', title: 'Mobile chains / collapsed, expanded, actionable / dark', kind: 'active', theme: 'dark', dataset: 'chains', activeSlug: null },
  { id: 'naming-context-dark', title: 'Mobile naming and context fallbacks / dark', kind: 'active', theme: 'dark', dataset: 'naming-context', activeSlug: null },
  { id: 'long-list-dark', title: 'Long mobile list / dense scrolling with active continuation / dark', kind: 'active', theme: 'dark', dataset: 'long-list', activeSlug: 'long-chain-current-12' },
  { id: 'archived-dark', title: 'Archived mobile list / dark', kind: 'archived', theme: 'dark', dataset: 'archived', activeSlug: null },
] as const satisfies readonly {
  id: string;
  title: string;
  kind: MobileConversationListScenarioKind;
  theme: MobileConversationListTheme;
  dataset: MobileConversationListFixtureDataset;
  activeSlug: string | null;
}[];

export type MobileConversationListScenarioId = (typeof mobileConversationListScenarioDefinitions)[number]['id'];

export interface MobileConversationListScenario {
  id: MobileConversationListScenarioId;
  title: string;
  kind: MobileConversationListScenarioKind;
  theme: MobileConversationListTheme;
  dataset: MobileConversationListFixtureDataset;
  activeSlug: string | null;
}

export interface MobileConversationListFixtureData {
  conversations: Conversation[];
  archivedConversations: Conversation[];
}
