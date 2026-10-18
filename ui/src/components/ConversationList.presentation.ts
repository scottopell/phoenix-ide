import type { ProductConversationListRow } from '../api';

export function productConversationDisplayTitle(row: ProductConversationListRow): string {
  return row.presentation.display_name.trim() || 'Conversation';
}

export function productConversationOwnsRoute(
  pathname: string,
  row: ProductConversationListRow,
): boolean {
  const match = pathname.match(/^\/(?:c|product-conversations)\/([^/]+)$/);
  if (!match?.[1]) return false;
  let routeIdentity: string;
  try {
    routeIdentity = decodeURIComponent(match[1]);
  } catch {
    return false;
  }
  return [
    row.product_conversation_id,
    row.canonical_root.slug,
    row.canonical_root.transcript_row_id,
    row.latest_transcript_row_id,
  ].includes(routeIdentity);
}

export function renamedProductConversationLocation(
  pathname: string,
  search: string,
  hash: string,
  previous: ProductConversationListRow,
  renamed: ProductConversationListRow,
): { pathname: string; search: string; hash: string } | null {
  if (!productConversationOwnsRoute(pathname, previous)) return null;

  const match = pathname.match(/^\/(?:c|product-conversations)\/([^/]+)$/);
  let identity: string;
  try {
    identity = decodeURIComponent(match![1]!);
  } catch {
    return null;
  }

  const params = new URLSearchParams(search);
  const matchedTranscriptId = [
    previous.canonical_root.transcript_row_id,
    previous.latest_transcript_row_id,
  ].find((id) => id === identity);
  if (matchedTranscriptId && !params.has('source_transcript')) {
    params.set('source_transcript', matchedTranscriptId);
  }
  const serializedSearch = params.toString();
  return {
    pathname: renamed.canonical_route,
    search: serializedSearch ? `?${serializedSearch}` : '',
    hash,
  };
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
