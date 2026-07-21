import type { CoordinatorScenario, CoordinatorScenarioId } from './types';

export const coordinatorScenarios = [
  { id: 'conversation-idle', title: 'Conversation idle', description: 'Populated transcript with inline briefing action and composer.', working: false },
  { id: 'conversation-working', title: 'Conversation working', description: 'Populated transcript with active queued controls.', working: true },
] as const satisfies readonly CoordinatorScenario[];

export function getCoordinatorScenario(id: CoordinatorScenarioId): CoordinatorScenario {
  const scenario = coordinatorScenarios.find((item) => item.id === id);
  if (!scenario) throw new Error(`Unknown Coordinator scenario: ${id}`);
  return scenario;
}
