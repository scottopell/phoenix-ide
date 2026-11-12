import { useEffect, useRef } from 'react';
import { api } from '../api';
import type { ProductConversationListResponse } from '../api';
import { notifyProductConversationListMayHaveChanged } from '../notifications';

const AUTO_READ_DEBOUNCE_MS = 400;

/**
 * Product conversations the user explicitly marked unread. While an id is in
 * this set, viewing that conversation does not advance its read cursor; the
 * suppression lifts the next time the conversation is opened.
 */
const autoReadSuppressed = new Set<string>();

export function isAutoReadSuppressed(productConversationId: string): boolean {
  return autoReadSuppressed.has(productConversationId);
}

export async function markProductConversationRead(productConversationId: string): Promise<void> {
  autoReadSuppressed.delete(productConversationId);
  await api.markProductConversationRead(productConversationId);
  notifyProductConversationListMayHaveChanged();
}

export async function markProductConversationUnread(productConversationId: string): Promise<void> {
  autoReadSuppressed.add(productConversationId);
  await api.markProductConversationUnread(productConversationId);
  notifyProductConversationListMayHaveChanged();
}

function autoMarkRead(productConversationId: string): void {
  if (autoReadSuppressed.has(productConversationId)) return;
  if (document.visibilityState !== 'visible') return;
  void api.markProductConversationRead(productConversationId)
    .then(() => notifyProductConversationListMayHaveChanged())
    .catch(() => { /* next view or new message retries */ });
}

/**
 * Keeps a viewed product conversation read: on open, whenever its latest
 * message changes, and when the tab becomes visible again.
 */
export function useAutoMarkProductConversationRead(
  productConversationId: string | null | undefined,
  latestMessageId: string | null | undefined,
): void {
  const timerRef = useRef<number | null>(null);

  useEffect(() => {
    if (!productConversationId) return;
    autoReadSuppressed.delete(productConversationId);
    autoMarkRead(productConversationId);
    const handleVisibility = () => autoMarkRead(productConversationId);
    document.addEventListener('visibilitychange', handleVisibility);
    return () => document.removeEventListener('visibilitychange', handleVisibility);
  }, [productConversationId]);

  useEffect(() => {
    if (!productConversationId || !latestMessageId) return;
    if (timerRef.current !== null) window.clearTimeout(timerRef.current);
    timerRef.current = window.setTimeout(() => {
      timerRef.current = null;
      autoMarkRead(productConversationId);
    }, AUTO_READ_DEBOUNCE_MS);
    return () => {
      if (timerRef.current !== null) {
        window.clearTimeout(timerRef.current);
        timerRef.current = null;
      }
    };
  }, [productConversationId, latestMessageId]);
}

export type CoordinatorUnread = 'marked' | 'new_activity' | null;

export function coordinatorUnread(response: Pick<ProductConversationListResponse, 'coordinator'>): CoordinatorUnread {
  const state = response.coordinator?.read_state;
  return state?.state === 'unread' ? state.reason : null;
}
