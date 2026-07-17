import type { AssociatedPrSummaryResponse } from '../api';
import type { ConversationPrStatusHandle } from '../hooks/useConversationPrStatus';

interface PrRailAvailability {
  actionablePrs: AssociatedPrSummaryResponse[];
  canRepresentActiveSelection: boolean;
  shouldRender: boolean;
}

function samePr(left: AssociatedPrSummaryResponse, right: AssociatedPrSummaryResponse): boolean {
  return left.repo_owner === right.repo_owner
    && left.repo_name === right.repo_name
    && left.pr_number === right.pr_number;
}

export function derivePrRailAvailability(
  handle: ConversationPrStatusHandle,
  isMobile: boolean,
): PrRailAvailability {
  const actionablePrs = handle.activeSelection?.associated_prs.filter(
    (pr) => pr.display_state === 'open' || pr.display_state === 'draft',
  ) ?? [];
  const activePr = handle.activePrSummary;
  const canRepresentActiveSelection = actionablePrs.length > 0
    && (!activePr || actionablePrs.some((pr) => samePr(pr, activePr)));

  return {
    actionablePrs,
    canRepresentActiveSelection,
    shouldRender: canRepresentActiveSelection && (isMobile || actionablePrs.length > 1),
  };
}
