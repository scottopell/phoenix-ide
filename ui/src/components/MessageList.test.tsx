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
      withConvContext(
        <MessageList
          messages={[makeMessage(1)]}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-a"
        />,
      ),
    );

    const main = container.querySelector('#main-area') as HTMLElement;
    const messages = container.querySelector('#messages') as HTMLElement;

    await waitFor(() => expect(main.scrollTop).toBe(450));
    triggerResize(messages, 500);
    expect(main.scrollTop).toBe(450);

    rerender(
      withConvContext(
        <MessageList
          messages={[makeMessage(2)]}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-b"
        />,
      ),
    );
    triggerResize(messages, 500);
    expect(main.scrollTop).toBe(1000);

    // Revisit conv-a with its original message present. Unit-anchor
    // restore looks up by message_id — the saved anchor's
    // topVisibleUnitKey ('msg-1') matches the rendered unit and the
    // restore lands the user back at the saved offset.
    rerender(
      withConvContext(
        <MessageList
          messages={[makeMessage(1)]}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-a"
        />,
      ),
    );

    await waitFor(() => expect(main.scrollTop).toBe(450));
    triggerResize(messages, 500);
    expect(main.scrollTop).toBe(450);
  });

  it('recomputes the current unit anchor on visibility-hidden even without a scroll event', () => {
    const { container } = render(
      withConvContext(
        <MessageList
          messages={[makeMessage(1), makeMessage(2), makeMessage(3)]}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-a"
        />,
      ),
    );

    const main = container.querySelector('#main-area') as HTMLElement;
    main.scrollTop = 250;

    Object.defineProperty(document, 'visibilityState', {
      configurable: true,
      get: () => 'hidden',
    });
    document.dispatchEvent(new Event('visibilitychange'));

    const saved = JSON.parse(localStorage.getItem('phoenix:msglist:anchor:conv-a') ?? 'null');
    expect(saved).toMatchObject({
      topVisibleUnitKey: 'msg-3',
      offsetWithinUnit: 50,
      unitCountAtSave: 3,
    });
  });

  it('keeps viewport stable when a visible pending message is acknowledged', async () => {
    const pending = {
      localId: 'local-ack-1',
      text: 'pending acknowledgement',
      images: [],
      timestamp: 1,
      status: 'pending' as const,
    };
    const historical = Array.from({ length: 20 }, (_, i) => makeMessage(i + 1, 'user'));

    Object.defineProperty(HTMLElement.prototype, 'scrollHeight', {
      configurable: true,
      get: () => 3000,
    });

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

    const main = container.querySelector('#main-area') as HTMLElement;
    await waitFor(() => expect(container.querySelector('[data-render-unit-key="local-ack-1"]')).not.toBeNull());
    act(() => {
      main.scrollTop = 2025;
      main.dispatchEvent(new Event('scroll', { bubbles: true }));
    });

    rerender(
      withConvContext(
        <MessageList
          messages={[
            ...historical,
            {
              ...makeMessage(21, 'user'),
              message_id: 'local-ack-1',
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

    await waitFor(() => expect(container.querySelector('[data-render-unit-key="local-ack-1"]')).not.toBeNull());
    expect(main.scrollTop).toBe(2025);
  });

  it('stays pinned to bottom when acknowledging a pending tail message', async () => {
    const pending = {
      localId: 'local-ack-bottom',
      text: 'pending bottom acknowledgement',
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
          conversationId="conv-ack-bottom"
        />,
      ),
    );

    const main = container.querySelector('#main-area') as HTMLElement;
    await waitFor(() => expect(container.querySelector('[data-render-unit-key="local-ack-bottom"]')).not.toBeNull());
    act(() => {
      main.scrollTop = 600;
      main.dispatchEvent(new Event('scroll', { bubbles: true }));
    });

    Object.defineProperty(HTMLElement.prototype, 'scrollHeight', {
      configurable: true,
      get: () => 1200,
    });

    rerender(
      withConvContext(
        <MessageList
          messages={[
            ...historical,
            {
              ...makeMessage(21, 'user'),
              message_id: 'local-ack-bottom',
              content: { text: pending.text },
            },
          ]}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-ack-bottom"
        />,
      ),
    );

    await waitFor(() => expect(main.scrollTop).toBe(1200));
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
