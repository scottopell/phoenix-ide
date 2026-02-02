import { useState, useRef, useEffect, KeyboardEvent } from 'react';

interface InputAreaProps {
  canSend: boolean;
  agentWorking: boolean;
  isCancelling: boolean;
  onSend: (text: string) => void;
  onCancel: () => void;
}

export function InputArea({ canSend, agentWorking, isCancelling, onSend, onCancel }: InputAreaProps) {
  const [text, setText] = useState('');
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
  }, [text]);

  const handleSend = () => {
    const trimmed = text.trim();
    if (!trimmed || !canSend) return;
    onSend(trimmed);
    setText('');
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  return (
    <footer id="input-area">
      <div id="input-container">
        <textarea
          ref={textareaRef}
          id="message-input"
          placeholder="Type a message..."
          rows={1}
          disabled={!canSend}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={handleKeyDown}
        />
        {agentWorking ? (
          <button
            id="cancel-btn"
            onClick={onCancel}
            disabled={isCancelling}
          >
            {isCancelling ? 'Cancelling...' : 'Cancel'}
          </button>
        ) : (
          <button
            id="send-btn"
            onClick={handleSend}
            disabled={!canSend}
          >
            Send
          </button>
        )}
      </div>
    </footer>
  );
}
