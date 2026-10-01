import type { Conversation, ProductConversationListRow } from '../../api';

export type MobileConversationListTheme = 'dark' | 'light';
export type MobileConversationListScenarioKind = 'active' | 'archived';
export type MobileConversationListFixtureDataset = 'overview' | 'chains' | 'naming-context' | 'product-conversations' | 'archived';

export const mobileConversationListScenarioDefinitions = [
  { id: 'active-overview-dark', title: 'Active mobile list / state and PR coverage / dark', kind: 'active', theme: 'dark', dataset: 'overview' },
  { id: 'active-overview-light', title: 'Active mobile list / state and PR coverage / light', kind: 'active', theme: 'light', dataset: 'overview' },
  { id: 'chains-dark', title: 'Mobile chains / collapsed, expanded, actionable / dark', kind: 'active', theme: 'dark', dataset: 'chains' },
  { id: 'naming-context-dark', title: 'Mobile naming and context fallbacks / dark', kind: 'active', theme: 'dark', dataset: 'naming-context' },
  { id: 'product-conversations-dark', title: 'Product conversations / Open and History / dark', kind: 'active', theme: 'dark', dataset: 'product-conversations' },
  { id: 'product-conversations-light', title: 'Product conversations / Open and History / light', kind: 'active', theme: 'light', dataset: 'product-conversations' },
  { id: 'product-conversations-history-dark', title: 'Product conversations / History / dark', kind: 'archived', theme: 'dark', dataset: 'product-conversations' },
  { id: 'product-conversations-history-light', title: 'Product conversations / History / light', kind: 'archived', theme: 'light', dataset: 'product-conversations' },
  { id: 'archived-dark', title: 'Archived mobile list / dark', kind: 'archived', theme: 'dark', dataset: 'archived' },
] as const satisfies readonly {
  id: string;
  title: string;
  kind: MobileConversationListScenarioKind;
  theme: MobileConversationListTheme;
  dataset: MobileConversationListFixtureDataset;
}[];

export type MobileConversationListScenarioId = (typeof mobileConversationListScenarioDefinitions)[number]['id'];

export interface MobileConversationListScenario {
  id: MobileConversationListScenarioId;
  title: string;
  kind: MobileConversationListScenarioKind;
  theme: MobileConversationListTheme;
  dataset: MobileConversationListFixtureDataset;
}

export interface MobileConversationListFixtureData {
  conversations: Conversation[];
  archivedConversations: Conversation[];
  productConversations?: ProductConversationListRow[];
  archivedProductConversations?: ProductConversationListRow[];
}
