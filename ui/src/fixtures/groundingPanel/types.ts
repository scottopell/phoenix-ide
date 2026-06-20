import type { McpServerStatus, SkillEntry, TaskEntry, WorkScopeInventory } from '../../api';
import type { FileItem } from '../../components/FileExplorer/FileTree';

export type GroundingPanelScenarioId =
  | 'full-dark'
  | 'full-light'
  | 'empty-dark'
  | 'errors-dark'
  | 'collapsed-dark'
  | 'narrow-dark'
  | 'skill-detail-dark'
  | 'task-detail-dark';

export type GroundingPanelScenarioKind = 'full' | 'empty' | 'errors' | 'collapsed' | 'skill-detail' | 'task-detail' | 'narrow';

export type GroundingPanelTheme = 'dark' | 'light';

export interface GroundingPanelScenario {
  id: GroundingPanelScenarioId;
  title: string;
  kind: GroundingPanelScenarioKind;
  theme: GroundingPanelTheme;
  rootPath: string;
  conversationId: string;
  scopeKey: string;
  branchName: string;
  activeSlug: string;
  width: number;
  collapsed: boolean;
}

export interface GroundingPanelFixtureData {
  files: Map<string, FileItem[]>;
  mcp: McpServerStatus[];
  skills: SkillEntry[];
  tasks: TaskEntry[];
  workScope: WorkScopeInventory;
  taskDetailMarkdown: string;
  skillDetailMarkdown: string;
}
