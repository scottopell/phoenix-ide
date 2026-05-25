import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, waitFor } from '@testing-library/react';
import type { ConversationState, Message } from '../api';
import { MessageList } from './MessageList';
import { MAX_RENDERED_UNITS, SENTINEL_ROOT_MARGIN } from '../hooks/useBottomAnchoredWindow';
import { ConversationContext } from '../conversation/ConversationContext';
import { ConversationStore } from '../conversation/ConversationStore';

// MessageList now subscribes to the conversation store for
// useStreamingStartedAt (session-stable key). Wrap renders in a
// minimal context — no driver, no polling — sufficient for the
// selector to short-circuit on absent atoms.
function withConvContext(ui: React.ReactElement): React.ReactElement {
  const store = new ConversationStore();
  return (
    <ConversationContext.Provider value={store}>
      {ui}
    </ConversationContext.Provider>
  );
}

vi.mock('./MessageComponents', () => ({
  UserMessage: ({ message }: { message: { sequence_id: number } }) => (
    <div className="message user" data-sequence-id={message.sequence_id} data-payload-kind="user">user</div>
  ),
  QueuedUserMessage: () => (
    <div className="message queued" data-payload-kind="pending">pending</div>
  ),
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

let scrollHeightDescriptor: PropertyDescriptor | undefined;
let clientHeightDescriptor: PropertyDescriptor | undefined;
let originalGetBCR: typeof HTMLElement.prototype.getBoundingClientRect | undefined;
let intersectionObservers: MockIntersectionObserver[] = [];

class MockIntersectionObserver {
  readonly rootMargin: string;
  private readonly callback: IntersectionObserverCallback;

  constructor(callback: IntersectionObserverCallback, options?: IntersectionObserverInit) {
    this.callback = callback;
    this.rootMargin = options?.rootMargin ?? '';
    intersectionObservers.push(this);
  }

  observe() {}
  disconnect() {}

  intersect() {
    this.callback(
      [{ isIntersecting: true } as IntersectionObserverEntry],
      this as unknown as IntersectionObserver,
    );
  }
}

class MockResizeObserver {
  observe() {}
  disconnect() {}
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
    intersectionObservers = [];
    scrollHeightDescriptor = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'scrollHeight');
    clientHeightDescriptor = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientHeight');
    vi.stubGlobal('ResizeObserver', MockResizeObserver);
    vi.stubGlobal('IntersectionObserver', MockIntersectionObserver);
    Object.defineProperty(HTMLElement.prototype, 'scrollHeight', {
      configurable: true,
      get: () => 1000,
    });
    Object.defineProperty(HTMLElement.prototype, 'clientHeight', {
      configurable: true,
      get: () => 400,
    });
    // Mock getBoundingClientRect so the unit-anchor scroll math (which
    // uses bounding rects to compute content-relative positions) works
    // in happy-dom. Default happy-dom returns zeros for everything and
    // doesn't relayout when scrollTop changes, so the math otherwise
    // produces stale results on subsequent visits. The simulation:
    // every element's top in viewport = -scrollTop of the scroll root
    // (i.e., element is at content position 0, viewport-shifted by the
    // current scroll). #main-area itself stays at viewport top=0.
    originalGetBCR = HTMLElement.prototype.getBoundingClientRect;
    HTMLElement.prototype.getBoundingClientRect = function (this: HTMLElement) {
      if (this.id === 'main-area') {
        return new DOMRect(0, 0, 0, 400);
      }
      const root = this.ownerDocument.getElementById('main-area');
      const st = root?.scrollTop ?? 0;
      const key = this.dataset?.['renderUnitKey'];
      const seq = key?.startsWith('msg-') ? Number(key.slice(4)) : 0;
      const contentTop = Number.isFinite(seq) && seq > 0 ? (seq - 1) * 100 : 0;
      return new DOMRect(0, contentTop - st, 0, 80);
    };
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    if (originalGetBCR) {
      HTMLElement.prototype.getBoundingClientRect = originalGetBCR;
    }
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

  it('pending → sent acknowledgement keeps a single keyed render unit (REQ-MLRU-001)', async () => {
    // The previous bug class: a pending_user TailUnit was promoted to a
    // user HistoricalUnit on ack, which required scroll compensation
    // (PR #152 hotfix). The new model emits pending_user as a
    // HistoricalUnit appended at the tail of historicalUnits, sharing
    // the eventual user unit's key (localId == message_id at ack). The
    // pending → sent transition is therefore a keyed in-place update
    // on a single DOM node — no cross-region promotion.
    const pending = {
      localId: 'msg-21',
      text: 'pending acknowledgement',
      images: [],
      timestamp: 1,
      status: 'pending' as const,
    };
    const historical = Array.from({ length: 20 }, (_, i) => makeMessage(i + 1, 'user'));

    const { container, rerender } = render(
      withConvContext(
        <MessageList
          messages={historical}
          pendingMessages={[pending]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-ack"
        />,
      ),
    );

    await waitFor(() => expect(container.querySelector('[data-render-unit-key="msg-21"]')).not.toBeNull());

    // Capture the wrapper DOM node + its initial payload kind before
    // the ack rerender. Same key + same node identity post-rerender is
    // what proves React reconciled in-place rather than unmount/remount.
    const wrapperBefore = container.querySelector('[data-render-unit-key="msg-21"]');
    expect(wrapperBefore).not.toBeNull();
    expect(wrapperBefore!.querySelector('[data-payload-kind]')?.getAttribute('data-payload-kind')).toBe('pending');

    rerender(
      withConvContext(
        <MessageList
          messages={[
            ...historical,
            {
              ...makeMessage(21, 'user'),
              message_id: 'msg-21',
              content: { text: pending.text },
            },
          ]}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-ack"
        />,
      ),
    );

    // The render-unit-key wrapper for `msg-21` survives the ack — same
    // node identity, payload kind swaps pending → user.
    await waitFor(() => {
      const nodes = container.querySelectorAll('[data-render-unit-key="msg-21"]');
      expect(nodes).toHaveLength(1);
      expect(nodes[0]!.querySelector('[data-payload-kind]')?.getAttribute('data-payload-kind')).toBe('user');
    });
    const wrapperAfter = container.querySelector('[data-render-unit-key="msg-21"]');
    expect(wrapperAfter).toBe(wrapperBefore);
  });

  it('bounds retained historical DOM while expanding upward', async () => {
    const historical = Array.from({ length: 100 }, (_, i) => makeMessage(i + 1, 'user'));

    const { container } = render(
      withConvContext(
        <MessageList
          messages={historical}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-a"
        />,
      ),
    );

    await waitFor(() => {
      expect(intersectionObservers.some((o) => o.rootMargin === SENTINEL_ROOT_MARGIN)).toBe(true);
    });

    for (let i = 0; i < 5; i++) {
      act(() => {
        intersectionObservers.find((o) => o.rootMargin === SENTINEL_ROOT_MARGIN)?.intersect();
      });
    }

    const renderedUnits = container.querySelectorAll('[data-render-unit-key]');
    expect(renderedUnits).toHaveLength(MAX_RENDERED_UNITS);
    expect(container.querySelector('[data-sequence-id="100"]')).toBeNull();
    expect(container.querySelectorAll('.message-collapsed-spacer')).toHaveLength(2);
  });

  it('windows renderable rows so recent tool results do not hide their owning agent row', () => {
    const historical = [
      ...Array.from({ length: 20 }, (_, i) => makeMessage(i + 1, 'user')),
      makeMessage(21, 'agent'),
      ...Array.from({ length: 20 }, (_, i) => makeMessage(22 + i, 'tool')),
    ];

    const { container } = render(
      withConvContext(
        <MessageList
          messages={historical}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-a"
        />,
      ),
    );

    expect(container.querySelector('[data-sequence-id="21"]')).not.toBeNull();
    expect(container.querySelectorAll('.message.agent')).toHaveLength(1);
  });
});
