import { createRef, forwardRef, useImperativeHandle, useLayoutEffect, useRef } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { render, waitFor, act } from '@testing-library/react';
import type { ConversationState, Message } from '../api';
import { MessageList } from './MessageList';
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

// Mock react-virtuoso as a passthrough that renders all items plus the
// Header slot. Real virtualization is tested in-browser via agent-browser
// smoke (acceptance criteria in tasks/60410). Unit tests focus on:
//   - render-unit construction (buildRenderUnits)
//   - per-unit component dispatch
//   - keyed in-place reconciliation across the pending → sent transition
vi.mock('react-virtuoso', () => ({
  Virtuoso: forwardRef(<T,>({
    data,
    itemContent,
    components,
    computeItemKey,
    scrollerRef,
  }: {
    data: T[];
    itemContent: (index: number, data: T) => React.ReactNode;
    components?: { Header?: React.ComponentType };
    computeItemKey?: (index: number, data: T) => React.Key;
    scrollerRef?: (ref: HTMLElement | Window | null) => void;
  }, ref: React.Ref<{ scrollToIndex: (options: unknown) => void }>) => {
    const Header = components?.Header;
    const containerRef = useRef<HTMLDivElement>(null);
    useImperativeHandle(ref, () => ({ scrollToIndex: vi.fn() }), []);
    useLayoutEffect(() => {
      scrollerRef?.(containerRef.current);
      return () => scrollerRef?.(null);
    }, [scrollerRef]);
    return (
      <div data-testid="mock-virtuoso" ref={containerRef}>
        {Header && <Header />}
        {data.map((item, i) => {
          const key = computeItemKey ? computeItemKey(i, item) : i;
          return <div key={key}>{itemContent(i, item)}</div>;
        })}
      </div>
    );
  }),
}));

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

  it('folds trailing tool messages into the owning agent_turn unit (REQ-MLRU-002)', () => {
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

    // buildRenderUnits consumes trailing tool messages into the owning
    // agent_turn's toolResultsByUseId map; there is exactly one agent
    // row, never standalone tool rows (REQ-MLRU-002).
    expect(container.querySelector('[data-sequence-id="21"]')).not.toBeNull();
    expect(container.querySelectorAll('.message.agent')).toHaveLength(1);
  });

  it('renders a 100-message conversation without throwing', () => {
    // The deleted spacer-based windowing layer had a separate test that
    // asserted a bounded number of rendered units + presence of spacers.
    // With virtuoso owning virtualization, that's a library concern;
    // here we only assert MessageList builds + dispatches render units
    // for a representative payload size. Real windowing behavior is
    // verified by agent-browser smoke against running Phoenix.
    const historical = Array.from({ length: 100 }, (_, i) => makeMessage(i + 1, 'user'));

    const { container } = render(
      withConvContext(
        <MessageList
          messages={historical}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-100"
        />,
      ),
    );

    expect(container.querySelectorAll('[data-render-unit-key]').length).toBe(100);
  });

  it('ignores stale chapter jump offset retries after a newer jump', () => {
    vi.useFakeTimers();
    try {
      const historical = Array.from({ length: 2 }, (_, i) => makeMessage(i + 1, 'user'));
      const listRef = createRef<React.ElementRef<typeof MessageList>>();
      const { container } = render(
        withConvContext(
          <MessageList
            ref={listRef}
            messages={historical}
            pendingMessages={[]}
            convState={idleState}
            onRetry={vi.fn()}
            onOpenFile={undefined}
            conversationId="conv-jumps"
          />,
        ),
      );

      const scroller = container.querySelector<HTMLElement>('#messages')!;
      scroller.scrollTop = 100;
      scroller.getBoundingClientRect = () => ({
        left: 0,
        right: 800,
        top: 36,
        bottom: 600,
        width: 800,
        height: 564,
        x: 0,
        y: 36,
        toJSON: () => ({}),
      } as DOMRect);
      const firstRow = container.querySelector<HTMLElement>('[data-render-unit-key="msg-1"]')!;
      firstRow.getBoundingClientRect = () => ({
        left: 0,
        right: 800,
        top: 20 + (100 - scroller.scrollTop),
        bottom: 80 + (100 - scroller.scrollTop),
        width: 800,
        height: 60,
        x: 0,
        y: 20 + (100 - scroller.scrollTop),
        toJSON: () => ({}),
      } as DOMRect);
      firstRow.querySelector<HTMLElement>('.message')!.getBoundingClientRect = firstRow.getBoundingClientRect;
      const secondRow = container.querySelector<HTMLElement>('[data-render-unit-key="msg-2"]')!;
      secondRow.getBoundingClientRect = () => ({
        left: 0,
        right: 800,
        top: 25 + (100 - scroller.scrollTop),
        bottom: 85 + (100 - scroller.scrollTop),
        width: 800,
        height: 60,
        x: 0,
        y: 25 + (100 - scroller.scrollTop),
        toJSON: () => ({}),
      } as DOMRect);
      secondRow.querySelector<HTMLElement>('.message')!.getBoundingClientRect = secondRow.getBoundingClientRect;

      act(() => {
        listRef.current?.scrollToUnitIndex(0);
      });
      act(() => {
        vi.advanceTimersByTime(60);
      });
      act(() => {
        listRef.current?.scrollToUnitIndex(1);
      });
      act(() => {
        vi.advanceTimersByTime(601);
      });

      expect(scroller.scrollTop).toBe(81);
      expect(firstRow.querySelector('.message')).not.toHaveClass('breadcrumb-highlight');
      expect(secondRow.querySelector('.message')).toHaveClass('breadcrumb-highlight');
    } finally {
      vi.useRealTimers();
    }
  });
});
