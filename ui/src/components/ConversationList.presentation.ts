import type { ProductConversationListRow } from '../api';

export function productConversationDisplayTitle(row: ProductConversationListRow): string {
  return row.canonical_root.title?.trim()
    || row.presentation.display_name?.trim()
    || row.canonical_root.slug?.trim()
    || row.canonical_root.transcript_row_id;
}

export type ProductConversationPresentationIndicator = {
  label: string;
  dotClass: string;
  ariaLabel: string;
};

export function productConversationPresentationIndicator(row: ProductConversationListRow): ProductConversationPresentationIndicator {
  if (row.presentation.kind === 'needs_action') {
    return { label: 'Needs action', dotClass: 'awaiting-approval', ariaLabel: 'Needs action' };
  }
  switch (row.presentation.presentation_mode) {
    case 'needs_action':
      return { label: 'Needs action', dotClass: 'awaiting-approval', ariaLabel: 'Needs action' };
    case 'working':
      return { label: 'Working', dotClass: 'working', ariaLabel: 'Working' };
    case 'error':
      return { label: 'Error', dotClass: 'error', ariaLabel: 'Error' };
    case 'done':
      return { label: 'Completed', dotClass: 'terminal', ariaLabel: 'Completed' };
    default:
      return row.lifecycle.state === 'history'
        ? { label: 'History', dotClass: 'terminal', ariaLabel: 'History' }
        : { label: 'Open', dotClass: 'idle', ariaLabel: 'Open' };
  }
}
