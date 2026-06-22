import type { Conversation } from '../../api';

export type ConversationPanelTheme = 'dark' | 'light';
export type ConversationPanelScenarioKind = 'expanded' | 'collapsed' | 'archived';

export const conversationPanelScenarioDefinitions = [
  { id: 'expanded-dark', title: 'Expanded sidebar / dark', kind: 'expanded', theme: 'dark', width: 360, collapsed: false },
  { id: 'expanded-light', title: 'Expanded sidebar / light', kind: 'expanded', theme: 'light', width: 360, collapsed: false },
  { id: 'collapsed-dark', title: 'Collapsed rail / dark', kind: 'collapsed', theme: 'dark', width: 56, collapsed: true },
  { id: 'narrow-dark', title: 'Narrow sidebar / dark', kind: 'expanded', theme: 'dark', width: 280, collapsed: false },
  { id: 'archived-dark', title: 'Archived rows / dark', kind: 'archived', theme: 'dark', width: 360, collapsed: false },
] as const satisfies readonly {
  id: string;
  title: string;
  kind: ConversationPanelScenarioKind;
  theme: ConversationPanelTheme;
  width: number;
  collapsed: boolean;
}[];

export type ConversationPanelScenarioId = (typeof conversationPanelScenarioDefinitions)[number]['id'];

export interface ConversationPanelScenario {
  id: ConversationPanelScenarioId;
  title: string;
  kind: ConversationPanelScenarioKind;
  theme: ConversationPanelTheme;
  width: number;
  collapsed: boolean;
}

export interface ConversationPanelFixtureData {
  conversations: Conversation[];
  archivedConversations: Conversation[];
  activeSlug: string;
}
