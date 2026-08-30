import type { ModelsResponse } from '../../api';

export type NewConversationScenarioId = 'ready-git-project';

export interface NewConversationScenario {
  id: NewConversationScenarioId;
  theme: 'dark' | 'light';
  cwd: string;
  draft: string;
  models: ModelsResponse;
}
