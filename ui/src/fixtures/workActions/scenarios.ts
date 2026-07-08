import type { PrStatusResponse } from '../../api';
import { workActionsScenarioDefinitions } from './types';
import type { WorkActionsScenario, WorkActionsScenarioId } from './types';

const PR_URL = 'https://github.com/example/phoenix/pull/35397';

function openPr(overrides: Partial<PrStatusResponse> = {}): PrStatusResponse {
  return {
    found: true,
    number: 35397,
    title: 'Stabilize work actions PR feedback primary',
    url: PR_URL,
    state: 'OPEN',
    draft: false,
    base: 'main',
    head: 'task-35397-stabilize-work-actions-pr-feedback-primary',
    display_state: 'open',
    refresh: {
      state: 'fresh',
      last_attempted_at: '2026-07-07T12:00:00Z',
      last_refreshed_at: '2026-07-07T12:00:00Z',
      stale: false,
    },
    updated_at: '2026-07-07T11:58:00Z',
    check_state: 'failing',
    work_change: { kind: 'clean' },
    ...overrides,
  };
}

function ready(prStatus: PrStatusResponse): WorkActionsScenario['prState'] {
  return { status: 'ready', prStatus };
}

const byId: Record<WorkActionsScenarioId, Omit<WorkActionsScenario, 'id' | 'title'>> = {
  'cached-open-stable': {
    description: 'Models the cached PR seed before the fresh request completes: Address feedback is already the primary, with Open PR as the safe secondary link-out.',
    convModeLabel: 'Work',
    phaseType: 'idle',
    continuedInConvId: null,
    canSendMessage: true,
    prState: ready(openPr({
      refresh: {
        state: 'unavailable',
        reason: 'command_failed',
        last_attempted_at: '2026-07-07T12:00:00Z',
        stale: true,
      },
      unavailable_reason: 'command_failed',
      check_state: 'passing',
    })),
  },
  'fresh-address-feedback': {
    description: 'Fresh open PR with actionable feedback freshness rendered on the Address feedback primary.',
    convModeLabel: 'Work',
    phaseType: 'idle',
    continuedInConvId: null,
    canSendMessage: true,
    prState: ready(openPr({
      feedback_freshness: { state: 'new', count: 3 },
      feedback_coverage: { kind: 'incomplete', surfaces: ['review_threads'] },
    })),
  },
  'passing-address-feedback-merge-secondary': {
    description: 'Green PR still keeps Address feedback primary; Merge on GitHub rides as a non-glowing secondary.',
    convModeLabel: 'Work',
    phaseType: 'idle',
    continuedInConvId: null,
    canSendMessage: true,
    prState: ready(openPr({
      check_state: 'passing',
      feedback_freshness: { state: 'edited', count: 1 },
    })),
  },
  'merged-clean-up': {
    description: 'Merged PR makes terminal cleanup the primary action.',
    convModeLabel: 'Work',
    phaseType: 'idle',
    continuedInConvId: null,
    canSendMessage: true,
    prState: ready(openPr({ display_state: 'merged', state: 'MERGED', check_state: 'passing' })),
  },
  'no-pr-dirty-review': {
    description: 'No PR and local dirty work routes to View Diff before cleanup.',
    convModeLabel: 'Work',
    phaseType: 'idle',
    continuedInConvId: null,
    canSendMessage: true,
    prState: ready({
      found: false,
      refresh: {
        state: 'not_found',
        last_attempted_at: '2026-07-07T12:00:00Z',
        last_refreshed_at: '2026-07-07T12:00:00Z',
        stale: false,
      },
      work_change: { kind: 'dirty_needs_review', reason: 'uncommitted_changes' },
    }),
  },
  'no-pr-create-pr': {
    description: 'No PR, but pushed GitHub work can offer an honest Create PR link as the primary.',
    convModeLabel: 'Work',
    phaseType: 'idle',
    continuedInConvId: null,
    canSendMessage: true,
    prState: ready({
      found: false,
      refresh: {
        state: 'not_found',
        last_attempted_at: '2026-07-07T12:00:00Z',
        last_refreshed_at: '2026-07-07T12:00:00Z',
        stale: false,
      },
      work_change: {
        kind: 'dirty_pr_ready',
        create_pr_url: 'https://github.com/example/phoenix/compare/main...task-35397?expand=1',
        branch_name: 'task-35397-stabilize-work-actions-pr-feedback-primary',
        base_branch: 'main',
      },
    }),
  },
  'gh-unavailable': {
    description: 'GitHub unavailable without a PR identity keeps cleanup available with a warning note.',
    convModeLabel: 'Work',
    phaseType: 'idle',
    continuedInConvId: null,
    canSendMessage: true,
    prState: ready({
      found: false,
      unavailable_reason: 'not_authenticated',
      refresh: {
        state: 'unavailable',
        reason: 'not_authenticated',
        last_attempted_at: '2026-07-07T12:00:00Z',
        stale: false,
      },
      work_change: { kind: 'unavailable', reason: 'gh auth required' },
    }),
  },
  'stuck-open-pr': {
    description: 'Error/context-exhausted phases suppress the Resolve zone entirely, even with an open PR.',
    convModeLabel: 'Work',
    phaseType: 'error',
    continuedInConvId: null,
    canSendMessage: true,
    prState: ready(openPr({ check_state: 'failing' })),
  },
};

export const workActionsScenarios: WorkActionsScenario[] = workActionsScenarioDefinitions.map((def) => ({
  ...def,
  ...byId[def.id],
}));

export function getWorkActionsScenario(id: string | null | undefined): WorkActionsScenario {
  return workActionsScenarios.find((scenario) => scenario.id === id) ?? workActionsScenarios[0]!;
}
