import type { Conversation, ProductConversationListRow, Project } from '../../api';

export type SidebarScenarioId =
  | 'expanded-all-active'
  | 'expanded-project-archived'
  | 'expanded-empty-project'
  | 'collapsed-overflow'
  | 'product-actions-continued';

export interface SidebarScenario {
  id: SidebarScenarioId;
  theme: 'dark' | 'light';
  collapsed: boolean;
  initialProjectId: string | null;
  activeSlug: string | null;
}

export interface SidebarFixtureData {
  projects: Project[];
  conversations: Conversation[];
  archivedConversations: Conversation[];
  productConversations?: ProductConversationListRow[];
}
