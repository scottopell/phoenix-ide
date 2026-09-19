import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Message } from '../api';
import { MessageReviewAction, MessageReviewEnabledContext } from './MessageReviewAction';
import { OPEN_MESSAGE_VIEWER_EVENT } from './MessageContextMenu';

const message: Message = {
  message_id: 'answer', sequence_id: 2, message_type: 'agent',
  conversation_id: 'earlier-row', created_at: '2026-09-19T12:00:00Z',
  content: [{ type: 'tool_use', id: 'read', name: 'bash', input: { command: 'pwd' } }],
  display_data: { productOccurrenceToken: 'earlier-row:answer' },
};

afterEach(cleanup);

describe('message review entry', () => {
  it('omits tool-only and blank messages but opens mixed prose messages with their occurrence identity', () => {
    const view = (content: Message['content']) => <MessageReviewEnabledContext.Provider value>
      <MessageReviewAction message={{ ...message, content }} />
    </MessageReviewEnabledContext.Provider>;
    const { rerender } = render(view(message.content));
    expect(screen.queryByRole('button', { name: 'Open message reviewer' })).toBeNull();
    rerender(view([{ type: 'text', text: '  \n ' }]));
    expect(screen.queryByRole('button', { name: 'Open message reviewer' })).toBeNull();
    rerender(view([...(Array.isArray(message.content) ? message.content : []), { type: 'text', text: 'Review this result.' }]));
    const opened = vi.fn();
    window.addEventListener(OPEN_MESSAGE_VIEWER_EVENT, opened);
    try {
      fireEvent.click(screen.getByRole('button', { name: 'Open message reviewer' }));
      expect(opened).toHaveBeenCalledOnce();
      expect(opened.mock.calls[0]?.[0].detail).toEqual({
        sequenceId: 2, messageId: 'answer', occurrenceToken: 'earlier-row:answer', presentation: 'pane',
      });
    } finally {
      window.removeEventListener(OPEN_MESSAGE_VIEWER_EVENT, opened);
    }
  });
});
