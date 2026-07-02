import '../index.css';
import { readFileSync } from 'node:fs';
import { createRef, forwardRef, useImperativeHandle, useLayoutEffect, useRef } from 'react';
import { describe, expect, it, vi, beforeEach } from 'vitest';
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
//
// The mock captures `totalListHeightChanged` and shares a `scrollToIndex`
// mock so tests can exercise the manual auto-follow callback (see the
// `handleTotalListHeightChanged` test group).
const virtuosoMock = {
  scrollToIndex: vi.fn(),
  totalListHeightChanged: null as ((height: number) => void) | null,
};

beforeEach(() => {
  virtuosoMock.scrollToIndex = vi.fn();
  virtuosoMock.totalListHeightChanged = null;
});

vi.mock('react-virtuoso', () => ({
  Virtuoso: forwardRef(<T, C>({
    data,
    context,
    itemContent,
    components,
    computeItemKey,
    scrollerRef,
    totalListHeightChanged,
  }: {
    data: T[];
    context?: C;
    itemContent: (index: number, data: T, context: C) => React.ReactNode;
    // Mirror the real component's typing: slot components receive `context`.
    components?: { Header?: React.ComponentType<{ context: C }> };
    computeItemKey?: (index: number, data: T) => React.Key;
    scrollerRef?: (ref: HTMLElement | Window | null) => void;
    totalListHeightChanged?: (height: number) => void;
  }, ref: React.Ref<{ scrollToIndex: (options: unknown) => void }>) => {
    const Header = components?.Header;
    const containerRef = useRef<HTMLDivElement>(null);
    if (totalListHeightChanged) {
      virtuosoMock.totalListHeightChanged = totalListHeightChanged;
    }
    useImperativeHandle(ref, () => ({ scrollToIndex: virtuosoMock.scrollToIndex }), []);
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

// Tests for the manual auto-follow callback (handleTotalListHeightChanged).
// Virtuoso is mocked as a passthrough, so these tests call the captured
// `totalListHeightChanged` callback directly and assert whether the shared
// `scrollToIndex` mock was called. This exercises the pin/no-pin logic,
// first-non-empty snap, and viewport-shrink handling without a real
// virtualization layer.
describe('handleTotalListHeightChanged', () => {
  function setupScroller(scroller: HTMLElement, opts: {
    scrollHeight: number;
    scrollTop: number;
    clientHeight: number;
  }) {
    Object.defineProperty(scroller, 'scrollHeight', { configurable: true, get: () => opts.scrollHeight });
    Object.defineProperty(scroller, 'scrollTop', { configurable: true, get: () => opts.scrollTop, set: () => {} });
    Object.defineProperty(scroller, 'clientHeight', { configurable: true, get: () => opts.clientHeight });
  }

  it('re-snaps to bottom when pinned and height grows', () => {
    const historical = Array.from({ length: 5 }, (_, i) => makeMessage(i + 1, 'user'));
    const { container } = render(
      withConvContext(
        <MessageList
          messages={historical}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-reSnap"
        />,
      ),
    );

    const scroller = container.querySelector<HTMLElement>('#messages')!;
    // Simulate: user at bottom, height grows from 500 to 600
    setupScroller(scroller, { scrollHeight: 600, scrollTop: 100, clientHeight: 500 });
    // First call seeds the baseline via the conversation-switch handler
    // (lastSeenConvIdRef starts undefined) — no snap on initial mount
    act(() => virtuosoMock.totalListHeightChanged?.(500));
    // Engage (downward wheel) so the assertion exercises the pin branch,
    // not the pre-engagement settle rescue.
    fireEvent.wheel(scroller, { deltaY: 50 });
    // Clear so we only observe the re-snap
    virtuosoMock.scrollToIndex.mockClear();
    // Second call: height grew, user was at bottom (oldFromBottom = 500 - 100 - 500 = -100 <= 100)
    setupScroller(scroller, { scrollHeight: 600, scrollTop: 100, clientHeight: 500 });
    act(() => virtuosoMock.totalListHeightChanged?.(600));

    expect(virtuosoMock.scrollToIndex).toHaveBeenCalled();
  });

  it('does NOT re-snap when scrolled up past threshold and height grows', () => {
    const historical = Array.from({ length: 5 }, (_, i) => makeMessage(i + 1, 'user'));
    const { container } = render(
      withConvContext(
        <MessageList
          messages={historical}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-no-yank"
        />,
      ),
    );

    const scroller = container.querySelector<HTMLElement>('#messages')!;
    // Seed baseline via the conversation-switch handler (no snap on mount)
    setupScroller(scroller, { scrollHeight: 1000, scrollTop: 0, clientHeight: 400 });
    act(() => virtuosoMock.totalListHeightChanged?.(1000));
    // The user got scrolled-up by scrolling — engagement releases the
    // pre-engagement settle rescue (which re-snaps unconditionally).
    fireEvent.wheel(scroller, { deltaY: -50 });
    // Clear so we only observe subsequent calls
    virtuosoMock.scrollToIndex.mockClear();

    // User scrolled up: scrollTop = 0, but content is tall (prevHeight = 1000)
    // oldFromBottom = 1000 - 0 - 400 = 600 > 100 — well past the threshold.
    // Height grows further, but user is scrolled up, so no re-snap.
    setupScroller(scroller, { scrollHeight: 1200, scrollTop: 0, clientHeight: 400 });
    act(() => virtuosoMock.totalListHeightChanged?.(1200));

    expect(virtuosoMock.scrollToIndex).not.toHaveBeenCalled();
  });

  it('snaps to bottom on first non-empty update after mounting empty', () => {
    // Mount with empty messages, then add messages
    const { container, rerender } = render(
      withConvContext(
        <MessageList
          messages={[]}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-empty-first"
        />,
      ),
    );

    // No content yet — callback should not snap
    const scroller = container.querySelector<HTMLElement>('#messages')!;
    setupScroller(scroller, { scrollHeight: 0, scrollTop: 0, clientHeight: 500 });
    act(() => virtuosoMock.totalListHeightChanged?.(0));
    expect(virtuosoMock.scrollToIndex).not.toHaveBeenCalled();

    // Messages arrive
    rerender(
      withConvContext(
        <MessageList
          messages={Array.from({ length: 5 }, (_, i) => makeMessage(i + 1, 'user'))}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-empty-first"
        />,
      ),
    );

    // First non-empty height measurement — should snap
    setupScroller(scroller, { scrollHeight: 600, scrollTop: 0, clientHeight: 500 });
    act(() => virtuosoMock.totalListHeightChanged?.(600));
    expect(virtuosoMock.scrollToIndex).toHaveBeenCalled();
  });

  it('does NOT snap on delayed height delta after conversation switch (no scroll-yank)', () => {
    // Mount conversation A with messages
    const historicalA = Array.from({ length: 5 }, (_, i) => makeMessage(i + 1, 'user'));
    const { container, rerender } = render(
      withConvContext(
        <MessageList
          messages={historicalA}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-A"
        />,
      ),
    );

    // Seed baseline for conversation A.
    // The first measurement goes through the conversation-switch handler
    // (lastSeenConvIdRef starts undefined), which seeds the baseline
    // WITHOUT scrolling — initialTopMostItemIndex already placed the
    // viewport for a conversation that mounted with messages.
    const scroller = container.querySelector<HTMLElement>('#messages')!;
    setupScroller(scroller, { scrollHeight: 500, scrollTop: 100, clientHeight: 400 });
    act(() => virtuosoMock.totalListHeightChanged?.(500));
    expect(virtuosoMock.scrollToIndex).not.toHaveBeenCalled();

    // Clear mock to track new calls
    virtuosoMock.scrollToIndex.mockClear();

    // Switch to conversation B (also has messages)
    const historicalB = Array.from({ length: 3 }, (_, i) => makeMessage(i + 10, 'user'));
    rerender(
      withConvContext(
        <MessageList
          messages={historicalB}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-B"
        />,
      ),
    );

    // Virtuoso re-keys on the conversationId change: the mock mounts a
    // FRESH scroller element, so re-query — the old `scroller` handle is
    // detached and mutating it would not affect what the component reads.
    const scrollerB = container.querySelector<HTMLElement>('#messages')!;
    setupScroller(scrollerB, { scrollHeight: 1000, scrollTop: 0, clientHeight: 400 });
    act(() => virtuosoMock.totalListHeightChanged?.(1000));
    // The conversation-switch handler seeds the baseline; should NOT snap
    // because hasSeenContentRef is seeded true (B already has messages)
    expect(virtuosoMock.scrollToIndex).not.toHaveBeenCalled();
    // The user scrolled up in B — engagement releases the settle rescue.
    fireEvent.wheel(scrollerB, { deltaY: -50 });

    // Now a delayed height delta arrives (e.g. code highlighter mount)
    // while the user is scrolled up in conversation B.
    // oldFromBottom = 1000 - 0 - 400 = 600 > 100
    setupScroller(scrollerB, { scrollHeight: 1100, scrollTop: 0, clientHeight: 400 });
    act(() => virtuosoMock.totalListHeightChanged?.(1100));
    // Should NOT snap — user is scrolled up, this is not a first-content case
    expect(virtuosoMock.scrollToIndex).not.toHaveBeenCalled();
  });

  it('re-snaps to bottom on pre-engagement height deltas even when stranded far from bottom', async () => {
    const historical = Array.from({ length: 5 }, (_, i) => makeMessage(i + 1, 'user'));
    const { container } = render(
      withConvContext(
        <MessageList
          messages={historical}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-stranded-mount"
        />,
      ),
    );

    const scroller = container.querySelector<HTMLElement>('#messages')!;
    // Mount stranding: virtuoso's initial LAST placement was computed
    // against early estimates; a huge correction lands and the viewport is
    // left at the top. The user has not interacted yet.
    setupScroller(scroller, { scrollHeight: 48000, scrollTop: 0, clientHeight: 600 });
    act(() => virtuosoMock.totalListHeightChanged?.(48000));

    // The settle snap writes scrollTop directly (a DOM snap cannot be
    // aborted by virtuoso's seek loop, unlike scrollToIndex). Record it.
    const written: number[] = [];
    Object.defineProperty(scroller, 'scrollHeight', { configurable: true, get: () => 12000000 });
    Object.defineProperty(scroller, 'scrollTop', {
      configurable: true,
      get: () => 0,
      set: (v: number) => written.push(v),
    });
    Object.defineProperty(scroller, 'clientHeight', { configurable: true, get: () => 600 });
    act(() => virtuosoMock.totalListHeightChanged?.(12000000));
    // The snap is deferred one frame so virtuoso's own compensation for
    // the triggering delta has already been applied.
    await act(() => new Promise((r) => requestAnimationFrame(() => r(undefined))));

    // Distance from bottom is enormous, but without user engagement the
    // list is still settling — it must recover to the bottom.
    expect(written).toContain(12000000);
  });

  it('settle watch corrects a silent stranding (no further events) within its window', () => {
    vi.useFakeTimers();
    try {
      const historical = Array.from({ length: 5 }, (_, i) => makeMessage(i + 1, 'user'));
      const { container } = render(
        withConvContext(
          <MessageList
            messages={historical}
            pendingMessages={[]}
            convState={idleState}
            onRetry={vi.fn()}
            onOpenFile={undefined}
            conversationId="conv-silent-strand"
          />,
        ),
      );

      const scroller = container.querySelector<HTMLElement>('#messages')!;
      // Silent stranding: after the first (seeding) measurement, virtuoso's
      // placement leaves the viewport off the bottom WITHOUT emitting any
      // further height delta or scroll event. Only the settle watch can see
      // this.
      const written: number[] = [];
      Object.defineProperty(scroller, 'scrollHeight', { configurable: true, get: () => 12000000 });
      Object.defineProperty(scroller, 'scrollTop', {
        configurable: true,
        get: () => 11951000, // ~48k off the bottom
        set: (v: number) => written.push(v),
      });
      Object.defineProperty(scroller, 'clientHeight', { configurable: true, get: () => 600 });
      act(() => virtuosoMock.totalListHeightChanged?.(12000000));

      act(() => vi.advanceTimersByTime(500));
      expect(written).toContain(12000000);

      // Engagement stops the watch: no more corrections after the user
      // takes over.
      fireEvent.touchStart(scroller, { touches: [{}] });
      written.length = 0;
      act(() => vi.advanceTimersByTime(1000));
      expect(written).toHaveLength(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it('user engagement releases the pre-engagement settle rescue', () => {
    const historical = Array.from({ length: 5 }, (_, i) => makeMessage(i + 1, 'user'));
    const { container } = render(
      withConvContext(
        <MessageList
          messages={historical}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-engaged"
        />,
      ),
    );

    const scroller = container.querySelector<HTMLElement>('#messages')!;
    setupScroller(scroller, { scrollHeight: 1000, scrollTop: 0, clientHeight: 400 });
    act(() => virtuosoMock.totalListHeightChanged?.(1000));
    // Any pointer interaction with the list counts as engagement.
    fireEvent.pointerDown(scroller);
    virtuosoMock.scrollToIndex.mockClear();

    // Scrolled-up user + height delta: no rescue, no snap.
    setupScroller(scroller, { scrollHeight: 1200, scrollTop: 0, clientHeight: 400 });
    act(() => virtuosoMock.totalListHeightChanged?.(1200));
    expect(virtuosoMock.scrollToIndex).not.toHaveBeenCalled();
  });

  it('follows a pinned user even when virtuoso model height disagrees with DOM scrollHeight', () => {
    const historical = Array.from({ length: 5 }, (_, i) => makeMessage(i + 1, 'user'));
    const { container } = render(
      withConvContext(
        <MessageList
          messages={historical}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-model-bias"
        />,
      ),
    );

    const scroller = container.querySelector<HTMLElement>('#messages')!;
    // User is 32px from the DOM bottom (600 - 168 - 400) — pinned. But
    // virtuoso's model total is 675: a +75 estimate bias, as accumulates on
    // long conversations with many unmeasured rows. A model-based pin check
    // computes 675 - 168 - 400 = 107 > 100 and wrongly drops auto-follow.
    setupScroller(scroller, { scrollHeight: 600, scrollTop: 168, clientHeight: 400 });
    act(() => virtuosoMock.totalListHeightChanged?.(675));
    // Engage (downward wheel: no upward-suppression armed) so the assertion
    // exercises the distance-based pin branch, not the settle rescue.
    fireEvent.wheel(scroller, { deltaY: 50 });
    virtuosoMock.scrollToIndex.mockClear();

    // Tail content arrives: model 675 -> 775, DOM 600 -> 700.
    setupScroller(scroller, { scrollHeight: 700, scrollTop: 168, clientHeight: 400 });
    act(() => virtuosoMock.totalListHeightChanged?.(775));

    // DOM-units pin check: 600 - 168 - 400 = 32 <= 100 — followed.
    expect(virtuosoMock.scrollToIndex).toHaveBeenCalled();
  });

  it('does NOT re-snap while a touch gesture is active, and resumes after it ends', () => {
    vi.useFakeTimers();
    try {
      const historical = Array.from({ length: 5 }, (_, i) => makeMessage(i + 1, 'user'));
      const { container } = render(
        withConvContext(
          <MessageList
            messages={historical}
            pendingMessages={[]}
            convState={idleState}
            onRetry={vi.fn()}
            onOpenFile={undefined}
            conversationId="conv-touch"
          />,
        ),
      );

      const scroller = container.querySelector<HTMLElement>('#messages')!;
      // Seed baseline: user pinned at bottom
      setupScroller(scroller, { scrollHeight: 500, scrollTop: 100, clientHeight: 400 });
      act(() => virtuosoMock.totalListHeightChanged?.(500));
      virtuosoMock.scrollToIndex.mockClear();

      // Finger goes down and starts dragging up — still within the pin
      // threshold (oldFromBottom = 500 - 80 - 400 = 20) when a
      // measurement-driven height delta lands.
      fireEvent.touchStart(scroller, { touches: [{}] });
      setupScroller(scroller, { scrollHeight: 600, scrollTop: 80, clientHeight: 400 });
      act(() => virtuosoMock.totalListHeightChanged?.(600));
      expect(virtuosoMock.scrollToIndex).not.toHaveBeenCalled();

      // Finger lifts; once the suppress window expires, a pinned user is
      // followed again.
      fireEvent.touchEnd(scroller, { touches: [] });
      act(() => vi.advanceTimersByTime(500));
      setupScroller(scroller, { scrollHeight: 700, scrollTop: 200, clientHeight: 400 });
      // oldFromBottom = 600 - 200 - 400 = 0 (pinned)
      act(() => virtuosoMock.totalListHeightChanged?.(700));
      expect(virtuosoMock.scrollToIndex).toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it('does NOT re-snap within the suppress window after an upward scroll (momentum/wheel)', () => {
    vi.useFakeTimers();
    try {
      const historical = Array.from({ length: 5 }, (_, i) => makeMessage(i + 1, 'user'));
      const { container } = render(
        withConvContext(
          <MessageList
            messages={historical}
            pendingMessages={[]}
            convState={idleState}
            onRetry={vi.fn()}
            onOpenFile={undefined}
            conversationId="conv-momentum"
          />,
        ),
      );

      const scroller = container.querySelector<HTMLElement>('#messages')!;
      setupScroller(scroller, { scrollHeight: 500, scrollTop: 100, clientHeight: 400 });
      // Establish the scroll-direction baseline at scrollTop=100 (the
      // detector compares against the last observed scrollTop).
      fireEvent.scroll(scroller);
      act(() => virtuosoMock.totalListHeightChanged?.(500));
      virtuosoMock.scrollToIndex.mockClear();

      // Momentum follows a real gesture: finger down + lift (engagement),
      // then the fling's upward scroll events with no finger down.
      fireEvent.touchStart(scroller, { touches: [{}] });
      fireEvent.touchEnd(scroller, { touches: [] });
      // scrollTop decreases (upward) — momentum after finger lift, a wheel
      // notch, or a scrollbar drag all look like this.
      setupScroller(scroller, { scrollHeight: 500, scrollTop: 60, clientHeight: 400 });
      fireEvent.scroll(scroller);

      // Height delta lands while still within the pin threshold
      // (oldFromBottom = 500 - 60 - 400 = 40) and within the window — no snap.
      setupScroller(scroller, { scrollHeight: 600, scrollTop: 60, clientHeight: 400 });
      act(() => virtuosoMock.totalListHeightChanged?.(600));
      expect(virtuosoMock.scrollToIndex).not.toHaveBeenCalled();

      // After the window expires with no further upward movement, a pinned
      // user is followed again.
      act(() => vi.advanceTimersByTime(500));
      setupScroller(scroller, { scrollHeight: 700, scrollTop: 200, clientHeight: 400 });
      // oldFromBottom = 600 - 200 - 400 = 0 (pinned)
      act(() => virtuosoMock.totalListHeightChanged?.(700));
      expect(virtuosoMock.scrollToIndex).toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it('downward scroll does not suppress the pinned re-snap', () => {
    const historical = Array.from({ length: 5 }, (_, i) => makeMessage(i + 1, 'user'));
    const { container } = render(
      withConvContext(
        <MessageList
          messages={historical}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-downward"
        />,
      ),
    );

    const scroller = container.querySelector<HTMLElement>('#messages')!;
    setupScroller(scroller, { scrollHeight: 500, scrollTop: 50, clientHeight: 400 });
    act(() => virtuosoMock.totalListHeightChanged?.(500));
    // Engage so the assertion exercises the pin branch, not the settle rescue.
    fireEvent.wheel(scroller, { deltaY: 50 });
    virtuosoMock.scrollToIndex.mockClear();

    // scrollTop increases (downward) — e.g. our own snap or the user heading
    // to the bottom. Must NOT suppress auto-follow.
    setupScroller(scroller, { scrollHeight: 500, scrollTop: 100, clientHeight: 400 });
    fireEvent.scroll(scroller);

    setupScroller(scroller, { scrollHeight: 600, scrollTop: 100, clientHeight: 400 });
    // oldFromBottom = 500 - 100 - 400 = 0 (pinned)
    act(() => virtuosoMock.totalListHeightChanged?.(600));
    expect(virtuosoMock.scrollToIndex).toHaveBeenCalled();
  });

  it('re-snaps when viewport shrinks while pinned', () => {
    const historical = Array.from({ length: 5 }, (_, i) => makeMessage(i + 1, 'user'));
    const { container } = render(
      withConvContext(
        <MessageList
          messages={historical}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-shrink"
        />,
      ),
    );

    const scroller = container.querySelector<HTMLElement>('#messages')!;
    // Seed baseline: user at bottom with tall viewport
    setupScroller(scroller, { scrollHeight: 800, scrollTop: 100, clientHeight: 700 });
    act(() => virtuosoMock.totalListHeightChanged?.(800));
    // oldFromBottom = 800 - 100 - 700 = 0 (pinned)
    // Engage (downward wheel) so the assertion exercises the pin branch,
    // not the pre-engagement settle rescue.
    fireEvent.wheel(scroller, { deltaY: 50 });

    // Clear so we only observe subsequent calls
    virtuosoMock.scrollToIndex.mockClear();

    // Viewport shrinks from 700 to 500 (200px shrink > 100px threshold)
    // Without viewport-shrink handling, oldFromBottom = 800 - 100 - 500 = 200 > 100
    // With shrink handling, uses prevClientHeight (700): oldFromBottom = 800 - 100 - 700 = 0 <= 100
    setupScroller(scroller, { scrollHeight: 800, scrollTop: 100, clientHeight: 500 });
    act(() => virtuosoMock.totalListHeightChanged?.(800));

    expect(virtuosoMock.scrollToIndex).toHaveBeenCalled();
  });
});
