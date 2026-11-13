import { useEffect, useRef } from 'react';
import { api } from '../api';
import type { ProductConversationListResponse } from '../api';
import { notifyProductConversationListMayHaveChanged } from '../notifications';

const OBSERVE_DEBOUNCE_MS = 400;

/** Explicit "mark read": clears an unread mark for every client. */
export async function markProductConversationRead(productConversationId: string): Promise<void> {
  await api.markProductConversationRead(productConversationId, 'acknowledged');
  notifyProductConversationListMayHaveChanged();
}

export async function markProductConversationUnread(productConversationId: string): Promise<void> {
  await api.markProductConversationUnread(productConversationId);
  notifyProductConversationListMayHaveChanged();
}

function userIsLooking(): boolean {
  return document.visibilityState === 'visible' && document.hasFocus();
}

/**
 * Keeps the viewed product conversation read. Opening it acknowledges it
 * (clearing an unread mark) the first time the user is actually looking;
 * afterwards new output is only observed, which advances the cursor but
 * leaves any unread mark — possibly set from another client — in place.
 */
export function useAutoMarkProductConversationRead(
  productConversationId: string | null | undefined,
  latestMessageId: string | null | undefined,
): void {
  const acknowledgedRef = useRef<string | null>(null);
  /** Latest message id the cursor was last advanced while viewing. */
  const advancedThroughRef = useRef<string | null | undefined>(undefined);
  const latestRef = useRef(latestMessageId);
  latestRef.current = latestMessageId;
  const timerRef = useRef<number | null>(null);

  const advance = useRef((id: string) => {
    if (!userIsLooking()) return;
    const acknowledged = acknowledgedRef.current === id;
    // Nothing new since the last advance: another request would be a no-op.
    if (acknowledged && advancedThroughRef.current === latestRef.current) return;
    const kind = acknowledged ? 'observed' : 'acknowledged';
    acknowledgedRef.current = id;
    advancedThroughRef.current = latestRef.current;
    void api.markProductConversationRead(id, kind)
      .then(() => notifyProductConversationListMayHaveChanged())
      .catch(() => {
        advancedThroughRef.current = undefined;
        if (kind === 'acknowledged' && acknowledgedRef.current === id) acknowledgedRef.current = null;
      });
  }).current;

  useEffect(() => {
    if (!productConversationId) return;
    acknowledgedRef.current = null;
    advancedThroughRef.current = undefined;
    advance(productConversationId);
    const handleAttention = () => advance(productConversationId);
    document.addEventListener('visibilitychange', handleAttention);
    window.addEventListener('focus', handleAttention);
    return () => {
      document.removeEventListener('visibilitychange', handleAttention);
      window.removeEventListener('focus', handleAttention);
    };
  }, [productConversationId, advance]);

  useEffect(() => {
    if (!productConversationId || !latestMessageId) return;
    timerRef.current = window.setTimeout(() => {
      timerRef.current = null;
      advance(productConversationId);
    }, OBSERVE_DEBOUNCE_MS);
    return () => {
      if (timerRef.current !== null) window.clearTimeout(timerRef.current);
      timerRef.current = null;
    };
  }, [productConversationId, latestMessageId, advance]);
}

export type CoordinatorUnread = 'marked' | 'new_activity' | null;

export function coordinatorUnread(response: Pick<ProductConversationListResponse, 'coordinator'>): CoordinatorUnread {
  const state = response.coordinator?.read_state;
  return state?.state === 'unread' ? state.reason : null;
}
