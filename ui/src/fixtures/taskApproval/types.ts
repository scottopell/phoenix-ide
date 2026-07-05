export type TaskApprovalTheme = 'dark' | 'light';

export const taskApprovalScenarioDefinitions = [
  { id: 'mobile-dark', title: 'Mobile task approval / dark', theme: 'dark' },
  { id: 'mobile-light', title: 'Mobile task approval / light', theme: 'light' },
] as const satisfies readonly {
  id: string;
  title: string;
  theme: TaskApprovalTheme;
}[];

export type TaskApprovalScenarioId = (typeof taskApprovalScenarioDefinitions)[number]['id'];

export interface TaskApprovalScenario {
  id: TaskApprovalScenarioId;
  title: string;
  theme: TaskApprovalTheme;
}
