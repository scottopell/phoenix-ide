import type { CoordinatorScenario, CoordinatorScenarioId } from './types';

export const coordinatorScenarios = [
  { id: 'conversation-idle', title: 'Conversation idle', description: 'Populated transcript and composer.', initialView: 'conversation', working: false, expanded: false, fleetError: false },
  { id: 'conversation-working', title: 'Conversation working', description: 'Populated transcript with active queued controls.', initialView: 'conversation', working: true, expanded: false, fleetError: false },
  { id: 'fleet-compact', title: 'Work compact', description: 'Attention-first work rows with deterministic query controls.', initialView: 'work', working: false, expanded: false, fleetError: false },
  { id: 'fleet-expanded', title: 'Work attention', description: 'Attention summary with active work rows.', initialView: 'work', working: false, expanded: false, fleetError: false },
  { id: 'fleet-error', title: 'Work error', description: 'Open-work failure remains recoverable.', initialView: 'work', working: false, expanded: false, fleetError: true },
] as const satisfies readonly CoordinatorScenario[];

export function getCoordinatorScenario(id: CoordinatorScenarioId): CoordinatorScenario {
  const scenario = coordinatorScenarios.find((item) => item.id === id);
  if (!scenario) throw new Error(`Unknown Coordinator scenario: ${id}`);
  return scenario;
}
