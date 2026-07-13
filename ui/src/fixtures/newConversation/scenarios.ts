import type { NewConversationScenario, NewConversationScenarioId } from './types';

export const newConversationScenarios = [
  {
    id: 'ready-git-project',
    theme: 'dark',
    cwd: '/Users/alex/projects/phoenix-ide',
    draft: 'Refine the mobile new-conversation layout while keeping the desktop workflow compact.',
    projects: [
      {
        id: 'project-phoenix',
        canonical_path: '/Users/alex/projects/phoenix-ide',
        main_ref: 'main',
        created_at: '2026-07-10T10:00:00Z',
        conversation_count: 12,
      },
      {
        id: 'project-design',
        canonical_path: '/Users/alex/projects/design-system',
        main_ref: 'main',
        created_at: '2026-07-12T10:00:00Z',
        conversation_count: 5,
      },
      {
        id: 'project-tools',
        canonical_path: '/Users/alex/projects/agent-tools',
        main_ref: 'main',
        created_at: '2026-07-11T10:00:00Z',
        conversation_count: 3,
      },
    ],
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
    branches: [
      { name: 'main', local: true, remote: true },
      { name: 'task-94004-new-page-qa-fixtures', local: true, remote: true },
      { name: 'mobile-layout-experiment', local: true, remote: false },
    ],
    currentBranch: 'task-94004-new-page-qa-fixtures',
    defaultBranch: 'main',
    tasks: [
      {
        id: '94004',
        priority: 'p2',
        status: 'in-progress',
        slug: 'new-page-qa-fixtures',
        path: '/Users/alex/projects/phoenix-ide/tasks/94004-p2-in-progress--new-page-qa-fixtures.md',
      },
    ],
  },
] as const satisfies readonly NewConversationScenario[];

export function getNewConversationScenario(id: NewConversationScenarioId): NewConversationScenario {
  const scenario = newConversationScenarios.find((item) => item.id === id);
  if (!scenario) throw new Error(`Unknown new-conversation scenario: ${id}`);
  return scenario;
}
