export interface CoordinatorScenario {
  id: CoordinatorScenarioId;
  title: string;
  description: string;
  working: boolean;
  connectionState: 'connected' | 'reconnecting' | 'offline';
}

export type CoordinatorScenarioId =
  | 'conversation-idle'
  | 'conversation-working'
  | 'transport-reconnecting'
  | 'transport-disconnected';
