import type { McpServerStatus, SkillEntry, TaskEntry, WorkScopeInventory } from '../../api';
import type { FileItem } from '../../components/FileExplorer/FileTree';

export type GroundingPanelScenarioKind = 'full' | 'empty' | 'errors' | 'collapsed' | 'skill-detail' | 'task-detail' | 'work' | 'narrow' | 'file-tree';

export type GroundingPanelTheme = 'dark' | 'light';

/** Canonical scenario list — the single source the id union, the built
 *  scenarios, and (transitively, via the stories and Ladle's manifest) the
 *  screenshot capture set all derive from. Add/remove a scenario here only. */
export const groundingPanelScenarioDefinitions = [
  { id: 'full-dark', title: 'Full / dark', kind: 'full', theme: 'dark', width: 360, collapsed: false },
  { id: 'full-light', title: 'Full / light', kind: 'full', theme: 'light', width: 360, collapsed: false },
  { id: 'empty-dark', title: 'Empty states', kind: 'empty', theme: 'dark', width: 360, collapsed: false },
  { id: 'errors-dark', title: 'Error states', kind: 'errors', theme: 'dark', width: 360, collapsed: false },
  { id: 'collapsed-dark', title: 'Collapsed rail', kind: 'collapsed', theme: 'dark', width: 360, collapsed: true },
  { id: 'work-dark', title: 'Work resources / dark', kind: 'work', theme: 'dark', width: 360, collapsed: false },
  { id: 'work-light', title: 'Work resources / light', kind: 'work', theme: 'light', width: 360, collapsed: false },
  { id: 'narrow-dark', title: 'Narrow panel', kind: 'narrow', theme: 'dark', width: 248, collapsed: false },
  { id: 'file-tree-dark', title: 'File tree nesting', kind: 'file-tree', theme: 'dark', width: 360, collapsed: false },
  { id: 'skill-detail-dark', title: 'Selected skill detail', kind: 'skill-detail', theme: 'dark', width: 360, collapsed: false },
  { id: 'task-detail-dark', title: 'Selected task detail', kind: 'task-detail', theme: 'dark', width: 360, collapsed: false },
] as const satisfies readonly {
  id: string;
  title: string;
  kind: GroundingPanelScenarioKind;
  theme: GroundingPanelTheme;
  width: number;
  collapsed: boolean;
}[];

export type GroundingPanelScenarioId = (typeof groundingPanelScenarioDefinitions)[number]['id'];

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
