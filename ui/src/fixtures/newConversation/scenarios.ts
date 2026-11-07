import type { NewConversationScenario, NewConversationScenarioId } from './types';

export const newConversationScenarios = [
  {
    id: 'ready-git-project',
    theme: 'dark',
    cwd: '/Users/alex/projects/phoenix-ide',
    draft: 'Refine the mobile new-conversation layout while keeping the desktop workflow compact.',
    models: {
      models: [
        {
          id: 'claude-sonnet-4-6',
          provider: 'anthropic',
          description: 'Balanced coding model',
          context_window: 200_000,
          recommended: true,
        },
        {
          id: 'gpt-5.4',
          provider: 'openai',
          description: 'General-purpose coding model',
          context_window: 200_000,
          recommended: true,
        },
      ],
      default: 'claude-sonnet-4-6',
      llm_configured: true,
      credential_status: 'valid',
    },
  },
  {
    id: 'crowded-repositories',
    theme: 'dark',
    cwd: '/Users/alex/projects/phoenix-ide',
    draft: 'Make the new-conversation page comfortable to use.',
    recentPaths: Array.from({ length: 20 }, (_, index) =>
      `/Users/alex/projects/${index === 0 ? 'phoenix-ide' : `long-named-repository-${String(index + 1).padStart(2, '0')}-for-crowded-layout-testing`}`),
    models: {
      models: [{ id: 'claude-sonnet-4-6', provider: 'anthropic', description: 'Balanced coding model', context_window: 200_000, recommended: true }],
      default: 'claude-sonnet-4-6',
      llm_configured: true,
      credential_status: 'valid',
    },
  },
] as const satisfies readonly NewConversationScenario[];

export function getNewConversationScenario(id: NewConversationScenarioId): NewConversationScenario {
  const scenario = newConversationScenarios.find((item) => item.id === id);
  if (!scenario) throw new Error(`Unknown new-conversation scenario: ${id}`);
  return scenario;
}
