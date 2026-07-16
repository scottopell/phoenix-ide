import type { CoordinatorScenario, CoordinatorScenarioId } from './types';

export const coordinatorScenarios = [
  { id: 'conversation-idle', title: 'Conversation idle', description: 'Populated transcript and composer.', initialView: 'conversation', working: false, expanded: false, fleetError: false },
  { id: 'conversation-working', title: 'Conversation working', description: 'Populated transcript with active queued controls.', initialView: 'conversation', working: true, expanded: false, fleetError: false },
  { id: 'fleet-compact', title: 'Fleet compact', description: 'Attention-first project rows.', initialView: 'fleet', working: false, expanded: false, fleetError: false },
  { id: 'fleet-expanded', title: 'Fleet expanded', description: 'Touch-friendly expanded audit metadata.', initialView: 'fleet', working: false, expanded: false, fleetError: false },
  { id: 'fleet-error', title: 'Fleet error', description: 'Fleet failure remains recoverable.', initialView: 'fleet', working: false, expanded: false, fleetError: true },
] as const satisfies readonly CoordinatorScenario[];

export function getCoordinatorScenario(id: CoordinatorScenarioId): CoordinatorScenario {
  const scenario = coordinatorScenarios.find((item) => item.id === id);
  if (!scenario) throw new Error(`Unknown Coordinator scenario: ${id}`);
  return scenario;
}
