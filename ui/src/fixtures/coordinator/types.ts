export interface CoordinatorScenario {
  id: CoordinatorScenarioId;
  title: string;
  description: string;
  working: boolean;
}

export type CoordinatorScenarioId =
  | 'conversation-idle'
  | 'conversation-working';
