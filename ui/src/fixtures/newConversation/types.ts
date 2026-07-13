import type { GitBranchEntry, ModelsResponse, TaskEntry } from '../../api';

export type NewConversationScenarioId = 'ready-git-project';

export interface NewConversationScenario {
  id: NewConversationScenarioId;
  theme: 'dark' | 'light';
  cwd: string;
  draft: string;
  recentDirs: string[];
  models: ModelsResponse;
  branches: GitBranchEntry[];
  currentBranch: string;
  defaultBranch: string;
  tasks: TaskEntry[];
}
