import type { ConversationState } from '../api';

export type RouteFocusDecision = 'pending' | 'focus-composer' | 'preserve-owner' | 'consumed';

export interface RouteFocusInputs {
  isDesktop: boolean;
  routeKey: string | null;
  routeSettled: boolean;
  browserSessionStateLoaded: boolean;
  archived: boolean;
  targetMessageId: string | undefined;
  activeFocusScope: string | null;
  viewerOwnsFocus: boolean;
  composerRenders: boolean;
  phase: ConversationState;
}

export interface RouteFocusState {
  routeKey: string | null;
  decision: RouteFocusDecision;
}

export type RouteFocusAction =
  | { type: 'route_observed'; next: RouteFocusState; continuesRouteKey?: string }
  | { type: 'focus_applied' }
  | { type: 'interaction_claimed'; routeKey: string | null };

type PhaseFocusDisposition = 'focus' | 'defer' | 'preserve-owner';

function phaseFocusDisposition(state: ConversationState): PhaseFocusDisposition {
  switch (state.type) {
    case 'idle':
    case 'llm_requesting':
    case 'seeded_llm_requesting':
    case 'tool_executing':
    case 'awaiting_sub_agents':
    case 'cancelling_tool':
    case 'cancelling_sub_agents':
      return 'focus';
    case 'error':
      return state.error?.can_user_resume ? 'focus' : 'preserve-owner';
    case 'awaiting_llm':
    case 'awaiting_continuation':
    case 'cancelling':
    case 'provisioning':
      return 'defer';
    case 'recoverable_continuation_failure':
    case 'awaiting_task_approval':
    case 'awaiting_user_response':
    case 'context_exhausted':
    case 'handed_off':
    case 'awaiting_recovery':
    case 'creation_failed':
    case 'creation_cancelled':
    case 'terminal':
      return 'preserve-owner';
    default:
      state satisfies never;
      return 'preserve-owner';
  }
}

const TRANSIENT_ROUTE_SCOPES = new Set([
  'question-panel',
  'task-approval',
  'fork-proposal-review',
]);

export function decideRouteFocus(inputs: RouteFocusInputs): RouteFocusState {
  const { routeKey } = inputs;
  if (!routeKey) return { routeKey: null, decision: 'consumed' };
  if (!inputs.isDesktop) return { routeKey, decision: 'preserve-owner' };
  if (!inputs.routeSettled || !inputs.browserSessionStateLoaded) return { routeKey, decision: 'pending' };
  if (
    inputs.archived
    || inputs.targetMessageId
    || (inputs.activeFocusScope !== null && !TRANSIENT_ROUTE_SCOPES.has(inputs.activeFocusScope))
    || inputs.viewerOwnsFocus
  ) {
    return { routeKey, decision: 'preserve-owner' };
  }
  if (inputs.activeFocusScope && TRANSIENT_ROUTE_SCOPES.has(inputs.activeFocusScope)) {
    return { routeKey, decision: 'pending' };
  }
  const phaseDecision = phaseFocusDisposition(inputs.phase);
  if (phaseDecision === 'preserve-owner') return { routeKey, decision: 'preserve-owner' };
  if (phaseDecision === 'defer' || !inputs.composerRenders) return { routeKey, decision: 'pending' };
  return { routeKey, decision: 'focus-composer' };
}

export function reduceRouteFocusState(current: RouteFocusState, action: RouteFocusAction): RouteFocusState {
  switch (action.type) {
    case 'route_observed':
      if (
        action.continuesRouteKey
        && current.routeKey === action.continuesRouteKey
        && current.decision === 'preserve-owner'
      ) {
        return { routeKey: action.next.routeKey, decision: 'preserve-owner' };
      }
      if (current.routeKey === action.next.routeKey && current.decision === 'consumed') return current;
      if (current.routeKey === action.next.routeKey && current.decision === 'preserve-owner') return current;
      if (current.routeKey === action.next.routeKey && current.decision === action.next.decision) return current;
      return action.next;
    case 'focus_applied':
      if (!current.routeKey || current.decision === 'consumed') return current;
      return { routeKey: current.routeKey, decision: 'consumed' };
    case 'interaction_claimed':
      if (!action.routeKey || current.routeKey !== action.routeKey || current.decision !== 'pending') return current;
      return { routeKey: current.routeKey, decision: 'preserve-owner' };
    default:
      action satisfies never;
      return current;
  }
}
