import type { ProductConversationListRow } from '../api';

export function productConversationDisplayTitle(row: ProductConversationListRow): string {
  return row.presentation.display_name.trim() || 'Conversation';
}

export function productConversationOwnsRoute(
  pathname: string,
  row: Pick<ProductConversationListRow, 'product_conversation_id' | 'canonical_root' | 'latest_transcript_row_id'>,
): boolean {
  const match = pathname.match(/^\/(?:c|product-conversations)\/([^/]+)\/?$/);
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
  previous: Pick<ProductConversationListRow, 'product_conversation_id' | 'canonical_root' | 'latest_transcript_row_id'>,
  renamed: ProductConversationListRow,
): { pathname: string; search: string; hash: string } | null {
  if (!productConversationOwnsRoute(pathname, previous)) return null;

  const match = pathname.match(/^\/(?:c|product-conversations)\/([^/]+)\/?$/);
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

/**
 * How long Ember holds a monitored conversation's question before it counts as
 * the user's again. A stuck Coordinator must not hide a question for good.
 */
export const COORDINATOR_QUESTION_GRACE_MS = 30 * 60_000;

/** True while Ember holds this conversation's current question (REQ-AUTH-009). */
export function questionWithCoordinator(row: ProductConversationListRow, now: number = Date.now()): boolean {
  const since = row.question_with_coordinator_since;
  if (!since) return false;
  const handedAt = Date.parse(since);
  return !Number.isNaN(handedAt) && now - handedAt < COORDINATOR_QUESTION_GRACE_MS;
}

/** When a held question becomes the user's again, or null if none is held. */
export function coordinatorQuestionExpiresAt(row: ProductConversationListRow): number | null {
  const since = row.question_with_coordinator_since;
  if (!since) return null;
  const handedAt = Date.parse(since);
  return Number.isNaN(handedAt) ? null : handedAt + COORDINATOR_QUESTION_GRACE_MS;
}

export function productConversationPresentationIndicator(
  row: ProductConversationListRow,
  now: number = Date.now(),
): ProductConversationPresentationIndicator {
  if (questionWithCoordinator(row, now)) {
    return { label: 'With Ember', dotClass: 'working', ariaLabel: 'Question with Ember' };
  }
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

/**
 * A row the user must act on; never folded away, whoever is monitoring it.
 * A question Ember holds is Ember's until the grace period runs out.
 */
export function productConversationNeedsUser(row: ProductConversationListRow, now: number = Date.now()): boolean {
  if (questionWithCoordinator(row, now)) return false;
  return row.presentation.kind === 'needs_action'
    || row.presentation.presentation_mode === 'needs_action'
    || row.presentation.presentation_mode === 'error';
}

export type MonitoredSplit = {
  /** Rows shown in the main list: the user's own, plus monitored rows that need the user. */
  own: readonly ProductConversationListRow[];
  /** Rows Ember is monitoring that need nothing from the user right now. */
  monitored: readonly ProductConversationListRow[];
};

/**
 * Separate conversations Ember monitors (`row.monitored`, an active Coordinator
 * watch) from the user's own. `pinned` keeps a row in the main list regardless,
 * e.g. the open one.
 */
export function splitMonitoredProductConversations(
  rows: readonly ProductConversationListRow[],
  pinned: (row: ProductConversationListRow) => boolean = () => false,
  now: number = Date.now(),
): MonitoredSplit {
  if (!rows.some((row) => row.monitored)) return { own: rows, monitored: [] };
  const own: ProductConversationListRow[] = [];
  const monitored: ProductConversationListRow[] = [];
  for (const row of rows) {
    const folded = row.monitored && !productConversationNeedsUser(row, now) && !pinned(row);
    (folded ? monitored : own).push(row);
  }
  return { own, monitored };
}
