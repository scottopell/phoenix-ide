import './MarkUnreadButton.css';
import { useEffect, useState } from 'react';
import { markProductConversationRead, markProductConversationUnread } from '../conversation/readState';
import { useProductConversationList } from '../conversation/productConversationList';

/**
 * Toggle for the conversation being viewed. Shows the server's explicit
 * unread mark (which another client may have set) rather than local state;
 * a pending toggle displays optimistically until the list refresh lands.
 */
export function MarkUnreadButton({ productConversationId, className }: {
  productConversationId: string;
  className?: string;
}) {
  const list = useProductConversationList();
  const readState = list.coordinator?.product_conversation_id === productConversationId
    ? list.coordinator.read_state
    : list.rows.find((row) => row.product_conversation_id === productConversationId)?.read_state;
  const serverMarked = readState?.state === 'unread' && readState.reason === 'marked';
  const [optimistic, setOptimistic] = useState<boolean | null>(null);
  const [pending, setPending] = useState(false);
  const [failed, setFailed] = useState(false);
  const marked = optimistic ?? serverMarked;

  useEffect(() => {
    setOptimistic(null);
    setFailed(false);
  }, [productConversationId, serverMarked]);

  const toggle = () => {
    const next = !marked;
    setPending(true);
    setFailed(false);
    setOptimistic(next);
    const action = next ? markProductConversationUnread : markProductConversationRead;
    action(productConversationId)
      .catch(() => {
        setOptimistic(null);
        setFailed(true);
      })
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
