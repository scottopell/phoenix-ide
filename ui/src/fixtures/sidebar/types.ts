import type { Conversation, ProductConversationListRow, Project } from '../../api';

export type SidebarScenarioId =
  | 'expanded-all-active'
  | 'expanded-project-archived'
  | 'expanded-empty-project'
  | 'collapsed-overflow'
  | 'product-actions-continued'
  | 'sidebar-visual-normal'
  | 'sidebar-visual-narrow';

export interface SidebarScenario {
  id: SidebarScenarioId;
  theme: 'dark' | 'light';
  collapsed: boolean;
  initialProjectId: string | null;
  activeSlug: string | null;
  sidebarWidth?: number;
}

export interface SidebarFixtureData {
  projects: Project[];
  conversations: Conversation[];
  archivedConversations: Conversation[];
  productConversations?: Partial<Record<SidebarScenarioId, ProductConversationListRow[]>>;
}
