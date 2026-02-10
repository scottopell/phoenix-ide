import { useState, useCallback, useEffect, useRef } from 'react';
import { generateUUID } from '../utils/uuid';
import type { ImageData } from '../api';

export type MessageStatus = 'sending' | 'failed';

export interface QueuedMessage {
  localId: string;
  text: string;
  images: ImageData[];
  timestamp: number;
  status: MessageStatus;
}

interface UseMessageQueueReturn {
  /** All queued messages (sending or failed) */
  queuedMessages: QueuedMessage[];
  /** Add a new message to the queue */
  enqueue: (text: string, images?: ImageData[]) => QueuedMessage;
  /** Mark a message as successfully sent (removes from queue) */
  markSent: (localId: string) => void;
  /** Mark a message as failed */
  markFailed: (localId: string) => void;
  /** Retry a failed message */
  retry: (localId: string) => void;
  /** Get messages ready to send */
  getPending: () => QueuedMessage[];
}

/**
 * Hook for managing a queue of unsent messages.
 * Messages persist to localStorage and survive page refresh.
 */
export function useMessageQueue(conversationId: string | undefined): UseMessageQueueReturn {
  const storageKey = conversationId ? `phoenix:queue:${conversationId}` : null;
  const initializedRef = useRef(false);

  // Load initial value from localStorage
  const loadFromStorage = useCallback((): QueuedMessage[] => {
    if (!storageKey) return [];
    try {
      const stored = localStorage.getItem(storageKey);
      return stored ? JSON.parse(stored) : [];
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
      status: 'sending',
    };
    updateMessages(prev => [...prev, msg]);
    return msg;
  }, [updateMessages]);

  // Mark a message as successfully sent (remove from queue)
  const markSent = useCallback((localId: string) => {
    updateMessages(prev => prev.filter(m => m.localId !== localId));
  }, [updateMessages]);

  // Mark a message as failed
  const markFailed = useCallback((localId: string) => {
    updateMessages(prev =>
      prev.map(m =>
        m.localId === localId ? { ...m, status: 'failed' as const } : m
      )
    );
  }, [updateMessages]);

  // Retry a failed message
  const retry = useCallback((localId: string) => {
    updateMessages(prev =>
      prev.map(m =>
        m.localId === localId ? { ...m, status: 'sending' as const } : m
      )
    );
  }, [updateMessages]);

  // Get messages that need to be sent
  const getPending = useCallback((): QueuedMessage[] => {
    return messages.filter(m => m.status === 'sending');
  }, [messages]);

  return {
    queuedMessages: messages,
    enqueue,
    markSent,
    markFailed,
    retry,
    getPending,
  };
}
