import type { CommissionReviewApprovalScope, Message } from '../../api';
import type { Theme } from '../../hooks/useTheme';

export interface CommissionReviewApprovalFixtureState {
  brief: string;
  focus: string | null;
  scope: CommissionReviewApprovalScope | undefined;
}

export interface CommissionReviewMessageFixtureState {
  message: Message;
  toolResults: Map<string, Message>;
  activeToolUseId?: string;
}

export interface CommissionReviewFixtureData {
  theme: Theme;
  approval: CommissionReviewApprovalFixtureState;
  inline: CommissionReviewMessageFixtureState;
}

export const commissionReviewScenarioDefinitions = [
  {
    id: 'approval-full-dark',
    title: 'Approval / full scope / dark',
    description: 'Realistic approval request with long repo/ref text and optional focus.',
    kind: 'approval-full',
    theme: 'dark',
  },
  {
    id: 'approval-missing-optional-dark',
    title: 'Approval / missing optional / dark',
    description: 'Approval request without optional focus and without loaded scope.',
    kind: 'approval-missing-optional',
    theme: 'dark',
  },
  {
    id: 'inline-running-dark',
    title: 'Inline / running / dark',
    description: 'Commission review tool call still running inline.',
    kind: 'inline-running',
    theme: 'dark',
  },
  {
    id: 'inline-clean-dark',
    title: 'Inline / clean / dark',
    description: 'Completed clean review with no findings.',
    kind: 'inline-clean',
    theme: 'dark',
  },
  {
    id: 'inline-findings-dark',
    title: 'Inline / findings / dark',
    description: 'Completed review with mixed-severity findings.',
    kind: 'inline-findings',
    theme: 'dark',
  },
  {
    id: 'inline-partial-dark',
    title: 'Inline / partial / dark',
    description: 'Partial review with warnings and unreviewed files.',
    kind: 'inline-partial',
    theme: 'dark',
  },
  {
    id: 'inline-failed-dark',
    title: 'Inline / failed / dark',
    description: 'Failed commission review with retry guidance.',
    kind: 'inline-failed',
    theme: 'dark',
  },
  {
    id: 'inline-rejected-dark',
    title: 'Inline / rejected / dark',
    description: 'Rejected commission review before token spend.',
    kind: 'inline-rejected',
    theme: 'dark',
  },
  {
    id: 'approval-full-light',
    title: 'Approval / full scope / light',
    description: 'Light-theme approval request with full scope.',
    kind: 'approval-full',
    theme: 'light',
  },
  {
    id: 'approval-missing-optional-light',
    title: 'Approval / missing optional / light',
    description: 'Light-theme approval request without optional scope/focus.',
    kind: 'approval-missing-optional',
    theme: 'light',
  },
  {
    id: 'inline-running-light',
    title: 'Inline / running / light',
    description: 'Light-theme running inline review.',
    kind: 'inline-running',
    theme: 'light',
  },
  {
    id: 'inline-clean-light',
    title: 'Inline / clean / light',
    description: 'Light-theme clean completed review.',
    kind: 'inline-clean',
    theme: 'light',
  },
  {
    id: 'inline-findings-light',
    title: 'Inline / findings / light',
    description: 'Light-theme findings summary.',
    kind: 'inline-findings',
    theme: 'light',
  },
  {
    id: 'inline-partial-light',
    title: 'Inline / partial / light',
    description: 'Light-theme partial review with warnings.',
    kind: 'inline-partial',
    theme: 'light',
  },
  {
    id: 'inline-failed-light',
    title: 'Inline / failed / light',
    description: 'Light-theme failed review.',
    kind: 'inline-failed',
    theme: 'light',
  },
  {
    id: 'inline-rejected-light',
    title: 'Inline / rejected / light',
    description: 'Light-theme rejected review.',
    kind: 'inline-rejected',
    theme: 'light',
  },
] as const satisfies readonly {
  id: string;
  title: string;
  description: string;
  kind:
    | 'approval-full'
    | 'approval-missing-optional'
    | 'inline-running'
    | 'inline-clean'
    | 'inline-findings'
    | 'inline-partial'
    | 'inline-failed'
    | 'inline-rejected';
  theme: Theme;
}[];

export type CommissionReviewScenarioId = (typeof commissionReviewScenarioDefinitions)[number]['id'];
export type CommissionReviewScenarioKind = (typeof commissionReviewScenarioDefinitions)[number]['kind'];

export interface CommissionReviewScenario {
  id: CommissionReviewScenarioId;
  title: string;
  description: string;
  kind: CommissionReviewScenarioKind;
  theme: Theme;
}
