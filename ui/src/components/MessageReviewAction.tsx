/* eslint-disable react-refresh/only-export-components -- shared message-review context */
import { createContext, useContext } from 'react';
import { MessageSquareText } from 'lucide-react';
import type { Message } from '../api';
import { getMessageMarkdown } from '../utils/messageCopy';
import { OPEN_MESSAGE_VIEWER_EVENT, type OpenMessageViewerEventDetail } from './MessageContextMenu';
import './MessageReviewAction.css';

export const MessageReviewEnabledContext = createContext(false);

export function MessageReviewAction({ message }: { message: Message }) {
  const enabled = useContext(MessageReviewEnabledContext);
  const data = message.display_data as { productOccurrenceToken?: string; productHistoricalHandoff?: unknown } | null;
  if (!enabled || data?.productHistoricalHandoff || !getMessageMarkdown(message).trim()) return null;
  return (
    <button
      type="button"
      className="message-review-action"
      aria-label="Open message reviewer"
      title="Open message reviewer"
      onClick={() => window.dispatchEvent(new CustomEvent<OpenMessageViewerEventDetail>(OPEN_MESSAGE_VIEWER_EVENT, {
        detail: {
          sequenceId: message.sequence_id,
          messageId: message.message_id,
          ...(data?.productOccurrenceToken ? { occurrenceToken: data.productOccurrenceToken } : {}),
          presentation: 'pane',
        },
      }))}
    >
      <MessageSquareText size={16} aria-hidden="true" />
      <span>Review</span>
    </button>
  );
}
