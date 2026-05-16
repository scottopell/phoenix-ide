import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, waitFor } from '@testing-library/react';
import type { ConversationState, Message } from '../api';
import { MessageList } from './MessageList';

vi.mock('./MessageComponents', () => ({
  UserMessage: ({ message }: { message: { sequence_id: number } }) => (
    <div className="message user" data-sequence-id={message.sequence_id}>user</div>
  ),
  QueuedUserMessage: () => null,
  AgentMessage: ({ message }: { message: { sequence_id: number } }) => (
    <div className="message agent" data-sequence-id={message.sequence_id}>agent</div>
  ),
  SubAgentStatus: () => null,
  formatMessageTime: () => '12:00',
}));

vi.mock('./StreamingMessage', () => ({
  StreamingMessage: () => null,
}));

vi.mock('./MessageContextMenu', () => ({
  MessageContextMenu: () => null,
}));

let resizeCallback: ResizeObserverCallback | null = null;

class MockResizeObserver {
  constructor(callback: ResizeObserverCallback) {
    resizeCallback = callback;
  }

  observe() {}
  disconnect() {}
}

function triggerResize(target: Element, height: number) {
  act(() => {
    resizeCallback?.(
      [{ target, contentRect: { height } as DOMRectReadOnly } as ResizeObserverEntry],
      {} as ResizeObserver,
    );
  });
}

function makeMessage(sequence_id: number): Message {
  return {
    message_id: `msg-${sequence_id}`,
    sequence_id,
    message_type: 'user',
    conversation_id: 'conv-under-test',
    content: { text: `message ${sequence_id}` },
    created_at: '2024-01-01T00:00:00Z',
  };
}

const idleState: ConversationState = { type: 'idle' };

describe('MessageList', () => {
  beforeEach(() => {
    localStorage.clear();
    resizeCallback = null;
    vi.stubGlobal('ResizeObserver', MockResizeObserver);
    Object.defineProperty(HTMLElement.prototype, 'scrollHeight', {
      configurable: true,
      get: () => 1000,
    });
    Object.defineProperty(HTMLElement.prototype, 'clientHeight', {
      configurable: true,
      get: () => 400,
    });
  });

  it('restores saved scroll when revisiting a cached conversation and does not snap back to bottom', async () => {
    localStorage.setItem('phoenix:scroll:conv-a', '450');
    localStorage.setItem('phoenix:msgcount:conv-a', '1');

    const { container, rerender } = render(
      <MessageList
        messages={[makeMessage(1)]}
        pendingMessages={[]}
        convState={idleState}
        onRetry={vi.fn()}
        onOpenFile={undefined}
        conversationId="conv-a"
        streamingBuffer={null}
      />,
    );

    const main = container.querySelector('#main-area') as HTMLElement;
    const messages = container.querySelector('#messages') as HTMLElement;

    await waitFor(() => expect(main.scrollTop).toBe(450));
    triggerResize(messages, 500);
    expect(main.scrollTop).toBe(450);

    rerender(
      <MessageList
        messages={[makeMessage(2)]}
        pendingMessages={[]}
        convState={idleState}
        onRetry={vi.fn()}
        onOpenFile={undefined}
        conversationId="conv-b"
        streamingBuffer={null}
      />,
    );
    triggerResize(messages, 500);
    expect(main.scrollTop).toBe(1000);

    rerender(
      <MessageList
        messages={[makeMessage(3)]}
        pendingMessages={[]}
        convState={idleState}
        onRetry={vi.fn()}
        onOpenFile={undefined}
        conversationId="conv-a"
        streamingBuffer={null}
      />,
    );

    await waitFor(() => expect(main.scrollTop).toBe(450));
    triggerResize(messages, 500);
    expect(main.scrollTop).toBe(450);
  });
});
