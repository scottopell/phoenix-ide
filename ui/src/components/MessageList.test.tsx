import '../index.css';
import { readFileSync } from 'node:fs';
import { createRef, forwardRef, useImperativeHandle, useLayoutEffect, useRef } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { render, waitFor, act, fireEvent } from '@testing-library/react';
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
  SkillCommandText: ({ text }: { text: string }) => {
    const [token = '', ...rest] = text.split(/\s+/);
    return (
      <span className="skill-command-inline">
        <span className="skill-command-chip"><span className="skill-command-slash">/</span><span className="skill-command-name">{token.replace(/^\//, '')}</span></span>
        {rest.length > 0 && <span className="skill-command-args"> {rest.join(' ')}</span>}
      </span>
    );
  },
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
  Virtuoso: forwardRef(<T, C>({
    data,
    context,
    itemContent,
    components,
    computeItemKey,
    scrollerRef,
  }: {
    data: T[];
    context?: C;
    itemContent: (index: number, data: T, context: C) => React.ReactNode;
    // Mirror the real component's typing: slot components receive `context`.
    components?: { Header?: React.ComponentType<{ context: C }> };
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
      <div
        data-testid="mock-virtuoso"
        ref={containerRef}
        style={{ overflowY: 'auto', height: '100%' }}
      >
        {Header && <Header context={context as C} />}
        {data.map((item, i) => {
          const key = computeItemKey ? computeItemKey(i, item) : i;
          return <div key={key}>{itemContent(i, item, context as C)}</div>;
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

const appCss = readFileSync(`${process.cwd()}/src/index.css`, 'utf8');

const idleState: ConversationState = { type: 'idle' };

describe('MessageList', () => {
  it('renders skill invocations as inline slash-command user messages with attachments', () => {
    const skillMessage = {
      ...makeMessage(7, 'skill'),
      content: {
        name: 'dogfood',
        trigger: '/dogfood http://localhost:8042',
        files: [
          {
            original_name: 'notes.txt',
            size_bytes: 512,
            stored_path: '/tmp/notes.txt',
          },
        ],
      },
    } as unknown as Message;

    const { container } = render(
      withConvContext(
        <MessageList
          messages={[skillMessage]}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-skill"
        />,
      ),
    );

    const message = container.querySelector('.message.user[data-sequence-id="7"]');
    expect(message).not.toBeNull();
    expect(message).toHaveTextContent('You');
    expect(message).toHaveTextContent('/dogfood http://localhost:8042');
    expect(message).toHaveTextContent('notes.txt');
    expect(message).toHaveTextContent('512 B');
    expect(message?.querySelector('.skill-command-name')).toHaveTextContent('dogfood');
    expect(message?.querySelector('.skill-command-args')).toHaveTextContent('http://localhost:8042');
    expect(container).not.toHaveTextContent('skill:');
    expect(container.querySelector('.skill-indicator')).toBeNull();
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
      conversationId: 'conv-1',
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

  it('makes the stamped Virtuoso scroller the chat route scroll owner', async () => {
    const historical = Array.from({ length: 100 }, (_, i) => makeMessage(i + 1, 'user'));

    const { container } = render(
      withConvContext(
        <MessageList
          messages={historical}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-scroll-owner"
        />,
      ),
    );

    const mainArea = container.querySelector<HTMLElement>('#main-area');
    await waitFor(() => expect(container.querySelector('#messages')).not.toBeNull());
    const messagesScroller = container.querySelector<HTMLElement>('#messages');

    expect(mainArea).not.toBeNull();
    expect(messagesScroller).not.toBeNull();
    expect(messagesScroller).toBe(container.querySelector('[data-testid="mock-virtuoso"]'));
    expect(mainArea).toHaveClass('chat-main-area');
    expect(appCss).toMatch(/#main-area\s*{[^}]*overflow:\s*hidden auto;/s);
    expect(appCss).toMatch(/#main-area\.chat-main-area\s*{[^}]*overflow:\s*hidden;/s);
    expect(appCss).toMatch(/\.desktop-main\s*{[^}]*overflow:\s*auto;/s);
    expect(appCss).toMatch(/\.desktop-main:has\(\.chat-main-area\)\s*{[^}]*overflow:\s*hidden;/s);
    expect(getComputedStyle(messagesScroller!).overflowY).toBe('auto');
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
      expect(firstRow.querySelector('.message')).not.toHaveClass('jump-highlight');
      expect(secondRow.querySelector('.message')).toHaveClass('jump-highlight');
    } finally {
      vi.useRealTimers();
    }
  });

  // Toggling the system prompt must not change the Virtuoso Header slot's
  // component *type*, only its props. A type swap forces Virtuoso to
  // unmount/remount the slot and recompute total list height — a visible
  // scroll hitch. We prove the absence of a remount by stamping a marker on
  // the live header node and asserting it survives the expand toggle.
  it('keeps the system-prompt Header slot mounted across expansion toggles', () => {
    const { container } = render(
      withConvContext(
        <MessageList
          messages={[makeMessage(1, 'user')]}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-sysprompt"
          systemPrompt="SENTINEL SYSTEM PROMPT"
        />,
      ),
    );

    const block = container.querySelector<HTMLElement>('.system-prompt-block');
    expect(block).not.toBeNull();
    // Collapsed initially: the prompt body is not in the DOM.
    expect(container.querySelector('.system-prompt-content')).toBeNull();

    // Stamp the live node; a remount would discard this DOM element.
    block!.dataset['persistMarker'] = 'kept';

    fireEvent.click(container.querySelector('.system-prompt-header')!);

    const afterToggle = container.querySelector<HTMLElement>('.system-prompt-block');
    expect(afterToggle?.dataset['persistMarker']).toBe('kept');
    expect(container.querySelector('.system-prompt-content')).toHaveTextContent(
      'SENTINEL SYSTEM PROMPT',
    );
  });
});
