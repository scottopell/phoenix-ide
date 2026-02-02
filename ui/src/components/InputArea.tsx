import { useRef, useEffect, KeyboardEvent } from 'react';
import type { QueuedMessage } from '../hooks';

interface InputAreaProps {
  draft: string;
  setDraft: (text: string) => void;
  canSend: boolean;
  agentWorking: boolean;
  isCancelling: boolean;
  isOffline: boolean;
  queuedMessages: QueuedMessage[];
  onSend: (text: string) => void;
  onCancel: () => void;
  onRetry: (localId: string) => void;
}

export function InputArea({
  draft,
  setDraft,
  canSend,
  agentWorking,
  isCancelling,
  isOffline,
  queuedMessages,
  onSend,
  onCancel,
  onRetry,
}: InputAreaProps) {
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const autoResize = () => {
    const ta = textareaRef.current;
    if (ta) {
      ta.style.height = 'auto';
      ta.style.height = Math.min(ta.scrollHeight, 120) + 'px';
    }
  };

  useEffect(() => {
    autoResize();
  }, [draft]);

  const handleSend = () => {
    const trimmed = draft.trim();
    if (!trimmed) return;
    // Allow sending even when offline - will be queued
    if (!canSend && !isOffline) return;
    onSend(trimmed);
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  // Show failed messages with retry option
  const failedMessages = queuedMessages.filter(m => m.status === 'failed');

  // Can send if: not working, OR offline (will queue)
  const sendEnabled = (canSend || isOffline) && draft.trim().length > 0;

  return (
    <footer id="input-area">
      {failedMessages.length > 0 && (
        <div className="failed-messages">
          {failedMessages.map(msg => (
            <div key={msg.localId} className="failed-message">
              <span className="failed-message-icon">⚠️</span>
              <span className="failed-message-text">
                Failed to send: "{msg.text.length > 50 ? msg.text.slice(0, 50) + '...' : msg.text}"
              </span>
              <button
                className="failed-message-retry"
                onClick={() => onRetry(msg.localId)}
              >
                Retry
              </button>
            </div>
          ))}
        </div>
      )}
      <div id="input-container">
        <textarea
          ref={textareaRef}
          id="message-input"
          placeholder={isOffline ? 'Type a message (will send when back online)...' : 'Type a message...'}
          rows={1}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={handleKeyDown}
        />
        {agentWorking ? (
          <button
            id="cancel-btn"
            onClick={onCancel}
            disabled={isCancelling || isOffline}
          >
            {isCancelling ? 'Cancelling...' : 'Cancel'}
          </button>
        ) : (
          <button
            id="send-btn"
            onClick={handleSend}
            disabled={!sendEnabled}
          >
            Send
          </button>
        )}
      </div>
    </footer>
  );
}
