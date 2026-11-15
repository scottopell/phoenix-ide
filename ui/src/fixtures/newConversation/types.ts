import type { ModelsResponse } from '../../api';

export type NewConversationScenarioId = 'ready-git-project' | 'crowded-repositories';

export interface NewConversationScenario {
  id: NewConversationScenarioId;
  theme: 'dark' | 'light';
  cwd: string;
  draft: string;
  recentPaths?: readonly string[];
  models: ModelsResponse;
}
