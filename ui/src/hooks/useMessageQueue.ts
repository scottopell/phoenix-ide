import { useState, useCallback } from 'react';
import { generateUUID } from '../utils/uuid';
import type { FileAttachment, ImageData } from '../api';

/**
 * A queued message is either:
 * - `pending`: the client has attempted (or will attempt) to send it, and it
 *   has not yet been echoed back by the server. Rendered in the message list.
 * - `failed`: the POST was rejected. Rendered in the input area with retry UI.
 * - `steering_queued`: the POST succeeded but the server queued the message
 *   because the conversation was busy. Rendered in the message list with a
 *   "Queued" indicator and a cancel button.
 *
 * "Sent" is NOT a state stored here — it is derived by comparing `localId`
 * against `atom.messages[*].message_id`. Once the server echoes the message,
 * the consumer filters it out of the rendered pending list automatically.
 */
export type MessageStatus = 'pending' | 'failed' | 'steering_queued';

export interface QueuedMessage {
  localId: string;
  text: string;
  images: ImageData[];
  files?: FileAttachment[];
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
    (q) => (q.status === 'pending' || q.status === 'steering_queued') && !serverIds.has(q.localId),
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
  enqueue: (text: string, images?: ImageData[], files?: FileAttachment[]) => QueuedMessage;
  /** Mark a message as failed. */
  markFailed: (localId: string) => void;
  /** Mark a message as steering_queued (server accepted but deferred). */
  markSteeringQueued: (localId: string) => void;
  /** Retry a failed message (transitions failed → pending). */
  retry: (localId: string) => void;
  /** Dismiss a message without retrying. Used for explicit user actions. */
  dismiss: (localId: string) => void;
}

function loadQueueFromStorage(storageKey: string | null): QueuedMessage[] {
  if (!storageKey) return [];
  try {
    const stored = localStorage.getItem(storageKey);
    if (!stored) return [];
    const parsed = JSON.parse(stored) as QueuedMessage[];
    // Coerce the legacy `'sending'` status to `'pending'` (renamed in task
    // 02676) so rehydrated entries survive the schema change without an
    // explicit migration path.
    return parsed.map((m) => {
      const rawStatus = (m as unknown as { status?: string }).status;
      if (rawStatus === 'sending') {
          return { ...m, files: (m as Partial<QueuedMessage>).files ?? [], status: 'pending' as const };
        }
        return { ...m, files: (m as Partial<QueuedMessage>).files ?? [] };
    });
  } catch (error) {
    console.warn('Error reading message queue from localStorage:', error);
    return [];
  }
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

  // In-render reset on conversationId change. The previous `useEffect`-based
  // reload committed the prior conversation's queue for one frame on
  // *returning* navigation (visit A → visit B → return to A): the render under
  // the new `conversationId` ran before the effect, so derived pending bubbles
  // from A briefly appeared in B's view. Reading storage during render — and
  // bumping a tracked-scope sentinel before returning — keeps state and props
  // in lockstep without a commit gap.
  const [messages, setMessages] = useState<QueuedMessage[]>(() => loadQueueFromStorage(storageKey));
  const [trackedConversationId, setTrackedConversationId] = useState<string | undefined>(conversationId);

  let currentMessages = messages;
  if (trackedConversationId !== conversationId) {
    setTrackedConversationId(conversationId);
    currentMessages = loadQueueFromStorage(storageKey);
    setMessages(currentMessages);
  }

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
  const enqueue = useCallback((text: string, images: ImageData[] = [], files: FileAttachment[] = []): QueuedMessage => {
    const msg: QueuedMessage = {
      localId: generateUUID(),
      text,
      images,
      files,
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

  // Mark a message as steering_queued (server accepted but deferred)
  const markSteeringQueued = useCallback((localId: string) => {
    updateMessages(prev =>
      prev.map(m =>
        m.localId === localId ? { ...m, status: 'steering_queued' as const } : m
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
    queuedMessages: currentMessages,
    enqueue,
    markFailed,
    markSteeringQueued,
    retry,
    dismiss,
  };
}
