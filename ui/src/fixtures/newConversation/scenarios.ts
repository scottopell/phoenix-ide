import type { NewConversationScenario, NewConversationScenarioId } from './types';

const sharedModels: NewConversationScenario['models'] = {
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
};

export const newConversationScenarios = [
  {
    id: 'ready-git-project',
    theme: 'dark',
    cwd: '/Users/alex/projects/phoenix-ide',
    draft: 'Refine the mobile new-conversation layout while keeping the desktop workflow compact.',
    models: sharedModels,
  },
  {
    id: 'recovery-staging',
    theme: 'dark',
    cwd: '/Users/alex/projects/phoenix-ide',
    draft: 'Retry the shipped recovery presentation without inventing new lifecycle behavior.',
    models: sharedModels,
    recoveryRows: [
      {
        request_id: 'req-delivery-failed',
        cwd: '/Users/alex/projects/phoenix-ide',
        objective: 'Publish the deterministic product conversation fixture capture slice.',
        model: 'claude-sonnet-4-6',
        effort: 'medium',
        status: 'delivery_failed',
        updated_at: '2026-07-01T11:42:00Z',
        published_product_conversation_id: null,
        llm_language: 'English',
        images: [],
        allowed_actions: ['retry_delivery'],
        last_error: 'The worktree was created, but publishing the product conversation route failed.',
      },
      {
        request_id: 'req-pending',
        cwd: '/Users/alex/projects/phoenix-ide',
        objective: '',
        model: 'gpt-5.4',
        effort: null,
        status: 'delivery_pending',
        updated_at: '2026-07-01T11:51:00Z',
        published_product_conversation_id: null,
        llm_language: 'English',
        images: [{ media_type: 'image/png', data: 'fixture-image' }],
        allowed_actions: ['retry_delivery'],
        last_error: null,
      },
    ],
    recoveryNextCursor: null,
  },
] as const satisfies readonly NewConversationScenario[];

export function getNewConversationScenario(id: NewConversationScenarioId): NewConversationScenario {
  const scenario = newConversationScenarios.find((item) => item.id === id);
  if (!scenario) throw new Error(`Unknown new-conversation scenario: ${id}`);
  return scenario;
}
