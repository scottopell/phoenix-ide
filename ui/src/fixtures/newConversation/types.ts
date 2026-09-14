import type { ModelsResponse, ProductConversationCreationRecoveryRow } from '../../api';

export type NewConversationScenarioId = 'ready-git-project' | 'recovery-staging';

export interface NewConversationScenario {
  id: NewConversationScenarioId;
  theme: 'dark' | 'light';
  cwd: string;
  draft: string;
  models: ModelsResponse;
  recoveryRows?: ProductConversationCreationRecoveryRow[];
  recoveryNextCursor?: string | null;
}
