export interface CoordinatorScenario {
  id: string;
  title: string;
  description: string;
  initialView: 'conversation' | 'work';
  working: boolean;
  expanded: boolean;
  fleetError: boolean;
}

export type CoordinatorScenarioId =
  | 'conversation-idle'
  | 'conversation-working'
  | 'fleet-compact'
  | 'fleet-expanded'
  | 'fleet-error';
