import './MarkUnreadButton.css';
import { useEffect, useState } from 'react';
import { markProductConversationRead, markProductConversationUnread } from '../conversation/readState';

/** Toggle for the conversation being viewed, which opening has just marked read. */
export function MarkUnreadButton({ productConversationId, className }: {
  productConversationId: string;
  className?: string;
}) {
  const [marked, setMarked] = useState(false);
  const [pending, setPending] = useState(false);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    setMarked(false);
    setFailed(false);
  }, [productConversationId]);

  const toggle = () => {
    setPending(true);
    setFailed(false);
    const next = !marked;
    const action = next ? markProductConversationUnread : markProductConversationRead;
    action(productConversationId)
      .then(() => setMarked(next))
      .catch(() => setFailed(true))
      .finally(() => setPending(false));
  };

  return (
    <button
      type="button"
      className={`mark-unread-btn${marked ? ' mark-unread-btn--marked' : ''}${className ? ` ${className}` : ''}`}
      onClick={toggle}
      disabled={pending}
      aria-pressed={marked}
      title={marked ? 'Marked unread — click to mark read' : 'Mark unread to revisit later'}
      data-testid="mark-unread-button"
    >
      <span className="mark-unread-btn__dot" aria-hidden="true" />
      {marked ? 'Unread' : 'Mark unread'}
      {failed && <span className="mark-unread-btn__error" role="status"> ✗</span>}
    </button>
  );
}
