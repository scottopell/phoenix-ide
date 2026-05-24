import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
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
let scrollHeightDescriptor: PropertyDescriptor | undefined;
let clientHeightDescriptor: PropertyDescriptor | undefined;

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

function makeMessage(sequence_id: number, message_type: Message['message_type'] = 'user'): Message {
  const content = message_type === 'tool'
    ? { tool_use_id: `tool-${sequence_id}`, content: 'tool result', is_error: false }
    : { text: `message ${sequence_id}` };
  return {
    message_id: `msg-${sequence_id}`,
    sequence_id,
    message_type,
    conversation_id: 'conv-under-test',
    content,
    created_at: '2024-01-01T00:00:00Z',
  } as Message;
}

const idleState: ConversationState = { type: 'idle' };

describe('MessageList', () => {
  beforeEach(() => {
    localStorage.clear();
    resizeCallback = null;
    scrollHeightDescriptor = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'scrollHeight');
    clientHeightDescriptor = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientHeight');
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

  afterEach(() => {
    vi.unstubAllGlobals();
    if (scrollHeightDescriptor) {
      Object.defineProperty(HTMLElement.prototype, 'scrollHeight', scrollHeightDescriptor);
    } else {
      delete (HTMLElement.prototype as unknown as { scrollHeight?: unknown }).scrollHeight;
    }
    if (clientHeightDescriptor) {
      Object.defineProperty(HTMLElement.prototype, 'clientHeight', clientHeightDescriptor);
    } else {
      delete (HTMLElement.prototype as unknown as { clientHeight?: unknown }).clientHeight;
    }
  });

  it('restores saved scroll when revisiting a cached conversation and does not snap back to bottom', async () => {
    // Unit-anchor restore: with one user message (key=msg-1) whose
    // wrapper has offsetTop=0 in happy-dom, an offset of 450 produces
    // scrollTop=450 — matching what the prior pixel-only model
    // recorded. The second navigation back to conv-a relies on the
    // save fired during the rerender writing this anchor back fresh.
    localStorage.setItem(
      'phoenix:msglist:anchor:conv-a',
      JSON.stringify({ topVisibleUnitKey: 'msg-1', offsetWithinUnit: 450 }),
    );

    const { container, rerender } = render(
      <MessageList
        messages={[makeMessage(1)]}
        pendingMessages={[]}
        convState={idleState}
        onRetry={vi.fn()}
        onOpenFile={undefined}
        conversationId="conv-a"
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
      />,
    );
    triggerResize(messages, 500);
    expect(main.scrollTop).toBe(1000);

    // Revisit conv-a with its original message present. Unit-anchor
    // restore looks up by message_id — the saved anchor's
    // topVisibleUnitKey ('msg-1') matches the rendered unit and the
    // restore lands the user back at the saved offset.
    rerender(
      <MessageList
        messages={[makeMessage(1)]}
        pendingMessages={[]}
        convState={idleState}
        onRetry={vi.fn()}
        onOpenFile={undefined}
        conversationId="conv-a"
      />,
    );

    await waitFor(() => expect(main.scrollTop).toBe(450));
    triggerResize(messages, 500);
    expect(main.scrollTop).toBe(450);
  });

  it('windows renderable rows so recent tool results do not hide their owning agent row', () => {
    const historical = [
      ...Array.from({ length: 20 }, (_, i) => makeMessage(i + 1, 'user')),
      makeMessage(21, 'agent'),
      ...Array.from({ length: 20 }, (_, i) => makeMessage(22 + i, 'tool')),
    ];

    const { container } = render(
      <MessageList
        messages={historical}
        pendingMessages={[]}
        convState={idleState}
        onRetry={vi.fn()}
        onOpenFile={undefined}
        conversationId="conv-a"
      />,
    );

    expect(container.querySelector('[data-sequence-id="21"]')).not.toBeNull();
    expect(container.querySelectorAll('.message.agent')).toHaveLength(1);
  });
});
