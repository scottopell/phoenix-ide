import type { CoordinatorScenario, CoordinatorScenarioId } from './types';

export const coordinatorScenarios = [
  { id: 'conversation-idle', title: 'Conversation idle', description: 'Populated transcript with inline briefing action and composer.', working: false, connectionState: 'connected', globalActivity: true },
  { id: 'conversation-working', title: 'Conversation working', description: 'Populated transcript with active queued controls.', working: true, connectionState: 'connected', globalActivity: true },
  { id: 'transport-reconnecting', title: 'Transport reconnecting', description: 'Compact Global StateBar with reconnecting transport and secondary activity.', working: false, connectionState: 'reconnecting', globalActivity: true },
  { id: 'transport-disconnected', title: 'Transport disconnected', description: 'Compact Global StateBar with disconnected transport and secondary activity.', working: false, connectionState: 'offline', globalActivity: true },
  { id: 'ordinary-reconnecting-frozen', title: 'Ordinary reconnecting frozen', description: 'Ordinary compact StateBar preserving last-known work while reconnecting.', working: true, connectionState: 'connected', globalActivity: false, freezeReconnectAfterMount: true },
  { id: 'global-watchdog-stale', title: 'Global watchdog stale', description: 'Global compact StateBar showing degraded no-signal health over connected transport.', working: true, connectionState: 'connected', globalActivity: true, staleWatchdog: true },
] as const satisfies readonly CoordinatorScenario[];

export function getCoordinatorScenario(id: CoordinatorScenarioId): CoordinatorScenario {
  const scenario = coordinatorScenarios.find((item) => item.id === id);
  if (!scenario) throw new Error(`Unknown Coordinator scenario: ${id}`);
  return scenario;
}
