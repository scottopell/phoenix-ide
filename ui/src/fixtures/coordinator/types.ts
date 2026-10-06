export interface CoordinatorScenario {
  id: CoordinatorScenarioId;
  title: string;
  description: string;
  working: boolean;
  connectionState: 'connected' | 'reconnecting' | 'offline';
  globalActivity: boolean;
  freezeReconnectAfterMount?: boolean;
}

export type CoordinatorScenarioId =
  | 'conversation-idle'
  | 'conversation-working'
  | 'transport-reconnecting'
  | 'transport-disconnected'
  | 'ordinary-reconnecting-frozen';
