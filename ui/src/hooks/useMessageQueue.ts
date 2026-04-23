import { useState, useCallback, useEffect, useRef } from 'react';
import { generateUUID } from '../utils/uuid';
import type { ImageData } from '../api';

/**
 * A queued message is either:
 * - `pending`: the client has attempted (or will attempt) to send it, and it
 *   has not yet been echoed back by the server. Rendered in the message list.
 * - `failed`: the POST was rejected. Rendered in the input area with retry UI.
 *
 * "Sent" is NOT a state stored here — it is derived by comparing `localId`
 * against `atom.messages[*].message_id`. Once the server echoes the message,
 * the consumer filters it out of the rendered pending list automatically.
 */
export type MessageStatus = 'pending' | 'failed';

export interface QueuedMessage {
  localId: string;
  text: string;
  images: ImageData[];
  timestamp: number;
  status: MessageStatus;
}

/**
 * Derive the list of pending messages to render in the conversation:
 * queue entries with status `pending` whose `localId` has NOT yet appeared
 * as a `message_id` in `atom.messages`.
 *
 * The server uses the client's `localId` as the canonical `message_id`, so
 * the join is deterministic. Once the SSE `message` echo arrives, the entry
 * filters out on the next render — no imperative `markSent` needed.
 */
export function derivePendingMessages(
  queuedMessages: QueuedMessage[],
  serverMessageIds: Iterable<string>,
): QueuedMessage[] {
  const serverIds = new Set(serverMessageIds);
  return queuedMessages.filter(
    (q) => q.status === 'pending' && !serverIds.has(q.localId),
  );
}

/**
 * Derive the list of failed messages to render in the input area.
 */
export function deriveFailedMessages(queuedMessages: QueuedMessage[]): QueuedMessage[] {
  return queuedMessages.filter((q) => q.status === 'failed');
}

interface UseMessageQueueReturn {
  /** All queued messages (pending or failed). Caller derives which to render where. */
  queuedMessages: QueuedMessage[];
  /** Add a new pending message to the queue. */
  enqueue: (text: string, images?: ImageData[]) => QueuedMessage;
  /** Mark a message as failed. */
  markFailed: (localId: string) => void;
  /** Retry a failed message (transitions failed → pending). */
  retry: (localId: string) => void;
  /** Dismiss a message without retrying. Used for explicit user actions. */
  dismiss: (localId: string) => void;
}

/**
 * Hook for managing a queue of messages the client has sent but the server
 * has not yet echoed. Messages persist to localStorage and survive page
 * refresh.
 *
 * Design: "sent" is not stored as a status. The consumer derives the rendered
 * pending list by filtering out `queuedMessages` whose `localId` appears in
 * `atom.messages[*].message_id` — the server uses the client's `localId` as
 * the canonical message id, so the join is deterministic. This eliminates
 * the timing gap between POST-success and SSE-echo that previously required
 * a reconciliation effect (task 02673 → 02676).
 */
export function useMessageQueue(conversationId: string | undefined): UseMessageQueueReturn {
  const storageKey = conversationId ? `phoenix:queue:${conversationId}` : null;
  const initializedRef = useRef(false);

  // Load initial value from localStorage. Coerce the legacy `'sending'` status
  // to `'pending'` (renamed in task 02676) so rehydrated entries survive the
  // schema change without an explicit migration path.
  const loadFromStorage = useCallback((): QueuedMessage[] => {
    if (!storageKey) return [];
    try {
      const stored = localStorage.getItem(storageKey);
      if (!stored) return [];
      const parsed = JSON.parse(stored) as QueuedMessage[];
      return parsed.map((m) => {
        const rawStatus = (m as unknown as { status?: string }).status;
        if (rawStatus === 'sending') {
          return { ...m, status: 'pending' as const };
        }
        return m;
      });
    } catch (error) {
      console.warn('Error reading message queue from localStorage:', error);
      return [];
    }
  }, [storageKey]);

  const [messages, setMessages] = useState<QueuedMessage[]>([]);

  // Initialize from storage when conversationId is available
  useEffect(() => {
    if (conversationId && !initializedRef.current) {
      setMessages(loadFromStorage());
      initializedRef.current = true;
    } else if (!conversationId) {
      setMessages([]);
      initializedRef.current = false;
    }
  }, [conversationId, loadFromStorage]);

  // Save to localStorage
  const saveToStorage = useCallback((msgs: QueuedMessage[]) => {
    if (!storageKey) return;
    try {
      if (msgs.length === 0) {
        localStorage.removeItem(storageKey);
      } else {
        localStorage.setItem(storageKey, JSON.stringify(msgs));
      }
    } catch (error) {
      console.warn('Error saving message queue to localStorage:', error);
    }
  }, [storageKey]);

  // Update state and storage together
  const updateMessages = useCallback((updater: (prev: QueuedMessage[]) => QueuedMessage[]) => {
    setMessages(prev => {
      const next = updater(prev);
      saveToStorage(next);
      return next;
    });
  }, [saveToStorage]);

  // Add a new message to the queue
  const enqueue = useCallback((text: string, images: ImageData[] = []): QueuedMessage => {
    const msg: QueuedMessage = {
      localId: generateUUID(),
      text,
      images,
      timestamp: Date.now(),
      status: 'pending',
    };
    updateMessages(prev => [...prev, msg]);
    return msg;
  }, [updateMessages]);

  // Mark a message as failed
  const markFailed = useCallback((localId: string) => {
    updateMessages(prev =>
      prev.map(m =>
        m.localId === localId ? { ...m, status: 'failed' as const } : m
      )
    );
  }, [updateMessages]);

  // Retry a failed message (flip back to pending; the send effect picks it up)
  const retry = useCallback((localId: string) => {
    updateMessages(prev =>
      prev.map(m =>
        m.localId === localId ? { ...m, status: 'pending' as const } : m
      )
    );
  }, [updateMessages]);

  // Dismiss a message (remove without retrying)
  const dismiss = useCallback((localId: string) => {
    updateMessages(prev => prev.filter(m => m.localId !== localId));
  }, [updateMessages]);

  return {
    queuedMessages: messages,
    enqueue,
    markFailed,
    retry,
    dismiss,
  };
}
