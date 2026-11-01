import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Message } from '../api';
import { AgentMessage, UserMessage } from './MessageComponents';
import { StreamingMessageView } from './StreamingMessage';
import { MessageReviewEnabledContext } from './MessageReviewAction';
import { OPEN_MESSAGE_VIEWER_EVENT } from './MessageContextMenu';
import { copyToClipboard } from '../utils/clipboard';

vi.mock('../utils/clipboard', () => ({ copyToClipboard: vi.fn().mockResolvedValue(true) }));
vi.mock('./MermaidDiagram', () => ({ MermaidDiagram: () => null }));

const text = 'Stable **assistant** prose.\n\nA second paragraph with enough text to wrap on a phone.';
const timestamp = '2026-10-15T12:00:00Z';
const message: Message = {
  message_id: 'finalization', conversation_id: 'chrome-test', sequence_id: 3,
  message_type: 'agent', content: [{ type: 'text', text }], created_at: timestamp,
  event_presentation: null, display_data: { productOccurrenceToken: 'occurrence-3' },
};
const buffer = { requestId: message.message_id, startedAt: Date.parse(timestamp), text, lastSequence: 1 };
const wrap = (node: React.ReactNode, review = true) => (
  <MemoryRouter><MessageReviewEnabledContext.Provider value={review}>{node}</MessageReviewEnabledContext.Provider></MemoryRouter>
);

beforeEach(() => { vi.useFakeTimers(); vi.clearAllMocks(); });
afterEach(() => { cleanup(); vi.useRealTimers(); });

describe('assistant finalization chrome', () => {
  it.each([true, false])('reserves the same toolbar before and after finalization (first=%s)', (isFirstInTurn) => {
    const { container, rerender } = render(wrap(<StreamingMessageView buffer={buffer} isFirstInTurn={isFirstInTurn} />));
    act(() => { vi.advanceTimersByTime(32); });
    const toolbarClass = container.querySelector('.assistant-message-toolbar')!.className;
    expect(container.querySelector('.message.agent')).toHaveClass('assistant-message');
    expect(container.querySelector('.assistant-message-actions')).toBeEmptyDOMElement();
    expect(screen.queryByRole('button', { name: 'Open message reviewer' })).toBeNull();
    expect(Boolean(container.querySelector('.message-sender'))).toBe(isFirstInTurn);
    const streamedText = container.querySelector('.message-content')!.textContent;

    rerender(wrap(<AgentMessage message={message} toolResults={new Map()} isFirstInTurn={isFirstInTurn} />));
    expect(container.querySelector('.message.agent')).toHaveClass('assistant-message');
    expect(container.querySelector('.assistant-message-toolbar')!.className).toBe(toolbarClass);
    expect(container.querySelector('.message-content')!.textContent).toBe(streamedText);
    expect(Boolean(container.querySelector('.message-sender'))).toBe(isFirstInTurn);
    expect(screen.getByRole('button', { name: 'Open message reviewer' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Copy Phoenix message' })).toBeInTheDocument();
    expect(container.querySelector('.message-header-actions')).toBeNull();
  });

  it.each([true, false])('retains the toolbar when actions are suppressed (first=%s)', (isFirstInTurn) => {
    const { container } = render(wrap(<AgentMessage message={message} toolResults={new Map()} isFirstInTurn={isFirstInTurn} suppressMessageCopy />));
    expect(container.querySelector('.assistant-message-toolbar')).toBeInTheDocument();
    expect(container.querySelector('.assistant-message-actions')).toBeEmptyDOMElement();
    expect(screen.queryByRole('button')).toBeNull();
  });

  it('keeps review optional without suppressing Copy or changing the toolbar', () => {
    const { container, rerender } = render(wrap(<AgentMessage message={message} toolResults={new Map()} />, false));
    const chrome = container.querySelector('.assistant-message-toolbar')!.className;
    expect(screen.queryByRole('button', { name: 'Open message reviewer' })).toBeNull();
    expect(screen.getByRole('button', { name: 'Copy Phoenix message' })).toBeInTheDocument();
    rerender(wrap(<AgentMessage message={{ ...message, display_data: { productHistoricalHandoff: true } }} toolResults={new Map()} />));
    expect(container.querySelector('.assistant-message-toolbar')!.className).toBe(chrome);
    expect(screen.queryByRole('button', { name: 'Open message reviewer' })).toBeNull();
    expect(screen.getByRole('button', { name: 'Copy Phoenix message' })).toBeInTheDocument();
  });

  it('preserves review occurrence identity and the copied markdown', async () => {
    const listener = vi.fn();
    window.addEventListener(OPEN_MESSAGE_VIEWER_EVENT, listener);
    try {
      render(wrap(<AgentMessage message={message} toolResults={new Map()} />));
      fireEvent.click(screen.getByRole('button', { name: 'Open message reviewer' }));
      expect(listener.mock.calls[0]![0].detail).toEqual({ sequenceId: 3, messageId: 'finalization', occurrenceToken: 'occurrence-3', presentation: 'pane' });
      await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Copy Phoenix message' })); });
      expect(copyToClipboard).toHaveBeenCalledWith(text);
    } finally {
      window.removeEventListener(OPEN_MESSAGE_VIEWER_EVENT, listener);
    }
  });

  it('does not apply assistant chrome to user messages or empty assistant messages', () => {
    const { container, rerender } = render(wrap(<UserMessage message={{ ...message, message_type: 'user', content: { text } }} />));
    expect(container.querySelector('.assistant-message')).toBeNull();
    expect(container.querySelector('.message-header-actions')).toBeInTheDocument();
    rerender(wrap(<AgentMessage message={{ ...message, content: [] }} toolResults={new Map()} />));
    expect(container.querySelector('.message')).toBeNull();
    rerender(wrap(<StreamingMessageView buffer={null} />));
    expect(container.querySelector('.message')).toBeNull();
  });
});
