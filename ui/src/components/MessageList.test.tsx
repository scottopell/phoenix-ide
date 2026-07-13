import '../index.css';
import { readFileSync } from 'node:fs';
import { createRef, forwardRef, StrictMode, useImperativeHandle, useLayoutEffect, useRef } from 'react';
import { FocusScopeProvider, useFocusScopeCommands } from '../hooks/useFocusScope';
import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, waitFor, act, fireEvent, screen } from '@testing-library/react';
import type { VirtualTranscriptPhysicalSnapshot, VirtualTranscriptRangeChange } from './VirtualTranscript';
import type { ConversationState, Message } from '../api';
import { MessageList } from './MessageList';
import type { HistoryScrollCommand } from '../conversation/historyExpansion';
import type { TranscriptPositioningInput } from '../conversation/transcriptPositioning';
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
      <FocusScopeProvider>{ui}</FocusScopeProvider>
    </ConversationContext.Provider>
  );
}

function PushScopeOnMount({ scopeId, children }: { scopeId: string; children: React.ReactNode }) {
  const { pushScope, popScope } = useFocusScopeCommands();
  useLayoutEffect(() => {
    pushScope(scopeId);
    return () => popScope(scopeId);
  }, [popScope, pushScope, scopeId]);
  return <>{children}</>;
}

// Render counter for the AgentMessage mock. The mock is memo()d like the
// real component, so this counts actual re-renders — the render-unit
// identity regression test (task 58044) asserts state ticks don't bump it.
const agentRenderCounter = vi.hoisted(() => ({ count: 0 }));
const agentMessageProps = vi.hoisted(
  () => [] as Array<{ message: Message; forceExpandedText: boolean | undefined; isLatestAgentMessage: boolean | undefined }>,
);

vi.mock('./MessageComponents', async () => {
  const React = await import('react');
  return {
    UserMessage: ({ message }: { message: { sequence_id: number } }) => (
      <div className="message user" data-sequence-id={message.sequence_id} data-payload-kind="user">user</div>
    ),
    QueuedUserMessage: () => (
      <div className="message queued" data-payload-kind="pending">pending</div>
    ),
    AgentMessage: React.memo(({ message, forceExpandedText, isLatestAgentMessage }: { message: Message; forceExpandedText?: boolean; isLatestAgentMessage?: boolean }) => {
      agentRenderCounter.count++;
      agentMessageProps.push({ message, forceExpandedText, isLatestAgentMessage });
      return <div className="message agent" data-sequence-id={message.sequence_id}>agent</div>;
    }),
    SubAgentStatus: ({ stateData }: { stateData: { pending: Array<{ task: string }>; completed_results: Array<{ task: string }> } }) => (
      <div data-testid="subagent-status-mock">
        {stateData.completed_results.map((agent, index) => <div key={`completed-${index}`}>{`completed ${agent.task}`}</div>)}
        {stateData.pending.map((agent, index) => <div key={`pending-${index}`}>{`pending ${agent.task}`}</div>)}
      </div>
    ),
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
  };
});

vi.mock('./StreamingMessage', () => ({
  StreamingMessage: () => null,
}));

vi.mock('./MessageContextMenu', () => ({
  MessageContextMenu: () => null,
}));

// Mock VirtualTranscript as a passthrough that renders all items plus the
// header. Real virtualization is covered by VirtualTranscript tests and render
// fixtures. Unit tests focus on:
//   - render-unit construction (buildRenderUnits)
//   - per-unit component dispatch
//   - keyed in-place reconciliation across the pending → sent transition
//
// The mock captures `onTotalExtentChange` and shares scroll command mocks so
// tests can exercise the manual auto-follow callback (see the
// `handleTotalListHeightChanged` test group).
const virtualTranscriptMock = {
  scrollToIndex: vi.fn(),
  scrollToTail: vi.fn(),
  captureVisibleAnchor: vi.fn(),
  measureOffsetForIndex: vi.fn(),
  measureOffsetForIndexAtSnapshot: vi.fn(),
  layoutRevision: vi.fn(),
  physicalSnapshot: vi.fn(),
  totalExtentChanged: null as ((height: number) => void) | null,
  pinnedChanged: null as ((pinned: boolean) => void) | null,
  rangeChanged: null as ((snapshot: VirtualTranscriptRangeChange) => void) | null,
  currentSnapshot: { renderedRange: null, visibleRange: null, viewportTop: 0, layoutRevision: 1 } as VirtualTranscriptPhysicalSnapshot,
  renderedIndices: null as Set<number> | null,
};

function indexOrZero(index: number): number {
  return index;
}

beforeEach(() => {
  virtualTranscriptMock.scrollToIndex = vi.fn();
  virtualTranscriptMock.scrollToTail = vi.fn();
  virtualTranscriptMock.captureVisibleAnchor = vi.fn(() => null);
  virtualTranscriptMock.measureOffsetForIndex = vi.fn(() => null);
  virtualTranscriptMock.measureOffsetForIndexAtSnapshot = vi.fn((index: number, snapshot: VirtualTranscriptPhysicalSnapshot) => snapshot.targetIndex === index ? snapshot.targetOffset ?? null : null);
  virtualTranscriptMock.layoutRevision = vi.fn(() => 1);
  virtualTranscriptMock.currentSnapshot = { renderedRange: null, visibleRange: null, viewportTop: 0, layoutRevision: 1 };
  virtualTranscriptMock.physicalSnapshot = vi.fn((targetIndex?: number) => targetIndex === undefined
    ? virtualTranscriptMock.currentSnapshot
    : { ...virtualTranscriptMock.currentSnapshot, targetIndex, targetOffset: virtualTranscriptMock.measureOffsetForIndex(indexOrZero(targetIndex)) });
  virtualTranscriptMock.totalExtentChanged = null;
  virtualTranscriptMock.pinnedChanged = null;
  virtualTranscriptMock.rangeChanged = null;
  virtualTranscriptMock.renderedIndices = null;
  agentRenderCounter.count = 0;
  agentMessageProps.length = 0;
});

vi.mock('./VirtualTranscript', async () => {
  const actual = await vi.importActual<typeof import('./VirtualTranscript')>('./VirtualTranscript');
  return {
    ...actual,
    VirtualTranscript: forwardRef(<T,>({
      items,
      renderItem,
      getKey,
      scrollerRef,
      onTotalExtentChange,
      onPinnedChange,
      onRangeChange,
      header,
      empty,
    }: {
      items: readonly T[];
      renderItem: (item: T, index: number) => React.ReactNode;
      getKey?: (item: T, index: number) => React.Key;
      scrollerRef?: (ref: HTMLDivElement | null) => void;
      onTotalExtentChange?: (height: number) => void;
      onPinnedChange?: (pinned: boolean) => void;
      onRangeChange?: (snapshot: VirtualTranscriptRangeChange) => void;
      header?: React.ReactNode;
      empty?: React.ReactNode;
    }, ref: React.Ref<{ scrollToIndex: (index: number, align: 'start' | 'end', viewportStartOffset?: number) => void; scrollToTail: () => void; captureVisibleAnchor: () => unknown; measureOffsetForIndex: (index: number) => number | null; measureOffsetForIndexAtSnapshot: (index: number, snapshot: VirtualTranscriptPhysicalSnapshot) => number | null; layoutRevision: () => number; physicalSnapshot: (targetIndex?: number) => VirtualTranscriptPhysicalSnapshot }>) => {
      const containerRef = useRef<HTMLDivElement>(null);
      if (onTotalExtentChange) {
        virtualTranscriptMock.totalExtentChanged = onTotalExtentChange;
      }
      if (onPinnedChange) {
        virtualTranscriptMock.pinnedChanged = onPinnedChange;
      }
      if (onRangeChange) {
        virtualTranscriptMock.rangeChanged = (snapshot) => {
          virtualTranscriptMock.currentSnapshot = snapshot;
          onRangeChange(snapshot);
        };
      }
      useImperativeHandle(ref, () => ({
        scrollToIndex: virtualTranscriptMock.scrollToIndex,
        scrollToTail: virtualTranscriptMock.scrollToTail,
        captureVisibleAnchor: virtualTranscriptMock.captureVisibleAnchor,
        measureOffsetForIndex: virtualTranscriptMock.measureOffsetForIndex,
        measureOffsetForIndexAtSnapshot: virtualTranscriptMock.measureOffsetForIndexAtSnapshot,
        layoutRevision: virtualTranscriptMock.layoutRevision,
        physicalSnapshot: virtualTranscriptMock.physicalSnapshot,
      }), []);
      useLayoutEffect(() => {
        scrollerRef?.(containerRef.current);
        return () => scrollerRef?.(null);
      }, [scrollerRef]);
      return (
        <div
          data-testid="mock-virtual-transcript"
          ref={containerRef}
          style={{ overflowY: 'auto', height: '100%' }}
        >
          {header}
          {items.length === 0 ? empty : items.map((item, i) => {
            if (virtualTranscriptMock.renderedIndices && !virtualTranscriptMock.renderedIndices.has(i)) return null;
            const key = getKey ? getKey(item, i) : i;
            return <div key={key}>{renderItem(item, i)}</div>;
          })}
        </div>
      );
    }),
  };
});

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

function makeRestoreAfterPrefixExpansionCommand(overrides: Partial<Extract<HistoryScrollCommand, { kind: 'restore_after_prefix_expansion' }>> = {}): Extract<HistoryScrollCommand, { kind: 'restore_after_prefix_expansion' }> {
  return {
    kind: 'restore_after_prefix_expansion',
    token: 1,
    requestToken: 11,
    view: { conversationId: 'conv-history', generation: 1, transcriptGeneration: 1 },
    messageId: 'msg-2',
    viewportStartOffset: -24,
    ...overrides,
  };
}


function transcriptPositioningForCommand(command?: HistoryScrollCommand | null): TranscriptPositioningInput {
  return command
    ? { kind: 'positioning', command }
    : { kind: 'idle', view: { conversationId: 'conv-history', generation: 1, transcriptGeneration: 1 } };
}

function makeJumpToMessageCommand(overrides: Partial<Extract<HistoryScrollCommand, { kind: 'jump_to_message' }>> = {}): Extract<HistoryScrollCommand, { kind: 'jump_to_message' }> {
  return {
    kind: 'jump_to_message',
    token: 1,
    requestToken: 11,
    view: { conversationId: 'conv-history', generation: 1, transcriptGeneration: 1 },
    targetMessageId: 'msg-2',
    ...overrides,
  };
}

describe('latest assistant expansion in compact mode', () => {
  it('shows the latest finalized assistant text fully in compact mode', () => {
    const messages: Message[] = [
      { ...makeMessage(1, 'user'), message_id: 'user-1', content: { text: 'Please summarize the plan.' } },
      { ...makeMessage(2, 'agent'), message_id: 'agent-1', content: [{ type: 'text', text: 'Older assistant summary that should collapse in compact mode.\n\nSecond line.' }] },
      { ...makeMessage(3, 'user'), message_id: 'user-2', content: { text: 'Anything else?' } },
      { ...makeMessage(4, 'agent'), message_id: 'agent-2', content: [{ type: 'text', text: 'Latest assistant summary should stay expanded in compact mode.\n\nSecond line.' }] },
    ];

    render(
      withConvContext(
        <MessageList
          messages={messages}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-latest-expanded"
          slug="conv-latest-expanded"

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
      ),
    );

    expect(agentMessageProps).toHaveLength(2);
    expect(agentMessageProps[0]?.forceExpandedText).toBe(false);
    expect(agentMessageProps[1]?.forceExpandedText).toBe(true);
    expect(agentMessageProps[0]?.isLatestAgentMessage).toBe(false);
    expect(agentMessageProps[1]?.isLatestAgentMessage).toBe(true);
  });
});

describe('MessageList', () => {
  it('claims find only while open, refocuses on repeat, and closes from transcript body', async () => {
    render(withConvContext(
      <MessageList
        messages={[makeMessage(1)]}
        pendingMessages={[]}
        convState={idleState}
        onRetry={vi.fn()}
        onOpenFile={undefined}
        conversationId="conv-find"
      />,
    ));

    const transcript = screen.getByTestId('mock-virtuoso');
    transcript.focus();
    fireEvent.keyDown(window, { key: 'f', metaKey: true });
    const input = await screen.findByRole('textbox', { name: 'Find in viewer' });
    expect(input).toHaveFocus();

    transcript.focus();
    expect(transcript).toHaveFocus();
    fireEvent.keyDown(window, { key: 'f', metaKey: true });
    await waitFor(() => expect(input).toHaveFocus());

    transcript.focus();
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(screen.queryByRole('textbox', { name: 'Find in viewer' })).toBeNull();
    await waitFor(() => expect(transcript).toHaveFocus());

    const escape = new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true });
    window.dispatchEvent(escape);
    expect(escape.defaultPrevented).toBe(false);
  });

  it('steps from the normalized transcript match index after results shrink', async () => {
    const initialMessages: Message[] = [
      { ...makeMessage(1, 'agent'), content: [{ type: 'text', text: 'alpha one' }] },
      { ...makeMessage(2, 'agent'), content: [{ type: 'text', text: 'alpha two' }] },
      { ...makeMessage(3, 'agent'), content: [{ type: 'text', text: 'alpha three' }] },
    ];
    const { rerender } = render(withConvContext(
      <MessageList
        messages={initialMessages}
        pendingMessages={[]}
        convState={idleState}
        onRetry={vi.fn()}
        onOpenFile={undefined}
        conversationId="conv-find-normalized"
      />,
    ));

    fireEvent.keyDown(window, { key: 'f', metaKey: true });
    const input = await screen.findByRole('textbox', { name: 'Find in viewer' });
    fireEvent.change(input, { target: { value: 'alpha' } });
    expect(screen.getByText('1 of 3')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByText('3 of 3')).toBeInTheDocument();

    rerender(withConvContext(
      <MessageList
        messages={initialMessages.slice(0, 2)}
        pendingMessages={[]}
        convState={idleState}
        onRetry={vi.fn()}
        onOpenFile={undefined}
        conversationId="conv-find-normalized"
      />,
    ));
    expect(screen.getByText('2 of 2')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByText('1 of 2')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Previous' }));
    expect(screen.getByText('2 of 2')).toBeInTheDocument();
  });

  it('searches expanded system prompt text when visible in the transcript header', async () => {
    render(withConvContext(
      <MessageList
        messages={[makeMessage(1)]}
        pendingMessages={[]}
        convState={idleState}
        onRetry={vi.fn()}
        onOpenFile={undefined}
        conversationId="conv-system-prompt-find"
        systemPrompt="alpha directive\nsecond line"
      />,
    ));

    fireEvent.click(document.querySelector('.system-prompt-header') as HTMLElement);
    fireEvent.keyDown(window, { key: 'f', metaKey: true });
    const input = await screen.findByRole('textbox', { name: 'Find in viewer' });
    fireEvent.change(input, { target: { value: 'alpha directive' } });

    expect(screen.getByText('1 of 1')).toBeInTheDocument();
  });

  it('reveals and highlights the expanded system prompt header match instead of scrolling to a row', async () => {
    render(withConvContext(
      <MessageList
        messages={[makeMessage(1)]}
        pendingMessages={[]}
        convState={idleState}
        onRetry={vi.fn()}
        onOpenFile={undefined}
        conversationId="conv-system-prompt-target"
        systemPrompt="alpha directive\nsecond line"
      />,
    ));

    fireEvent.click(document.querySelector('.system-prompt-header') as HTMLElement);
    fireEvent.keyDown(window, { key: 'f', metaKey: true });
    const input = await screen.findByRole('textbox', { name: 'Find in viewer' });
    fireEvent.change(input, { target: { value: 'alpha directive' } });

    const header = document.querySelector('.system-prompt-content') as HTMLElement;
    await waitFor(() => expect(header).toHaveClass('viewer-find-row-match--active'));
  });

  it('does not re-scroll the active transcript match on unrelated streaming-buffer ticks', async () => {
    const store = new ConversationStore();
    const messages = [
      { ...makeMessage(1, 'agent'), content: [{ type: 'text', text: 'alpha earlier match' }] },
      { ...makeMessage(2, 'user'), content: { text: 'later row' } },
    ] as Message[];

    render(
      <ConversationContext.Provider value={store}>
        <FocusScopeProvider>
          <MessageList
            slug="conv-stream-find"
            messages={messages}
            pendingMessages={[]}
            convState={idleState}
            onRetry={vi.fn()}
            onOpenFile={undefined}
            conversationId="conv-stream-find"
          />
        </FocusScopeProvider>
      </ConversationContext.Provider>,
    );

    fireEvent.keyDown(window, { key: 'f', metaKey: true });
    const input = await screen.findByRole('textbox', { name: 'Find in viewer' });
    fireEvent.change(input, { target: { value: 'alpha' } });

    await waitFor(() => expect(screen.getByText('1 of 1')).toBeInTheDocument());

    const activeRow = document.querySelector('[data-render-unit-key="msg-1"]') as HTMLElement;
    expect(activeRow).not.toBeNull();
    activeRow.scrollIntoView = vi.fn();

    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByText('1 of 1')).toBeInTheDocument();
    activeRow.scrollIntoView = vi.fn();

    act(() => {
      store.dispatch('conv-stream-find', {
        type: 'sse_token',
        sequenceId: 99,
        requestId: 'req-1',
        delta: 'token',
      });
    });

    expect(activeRow.scrollIntoView).not.toHaveBeenCalled();
    expect(screen.getByText('1 of 1')).toBeInTheDocument();
  });

  it('does not build transcript matches while find is closed or queryless', async () => {
    render(withConvContext(
      <MessageList
        messages={[makeMessage(1)]}
        pendingMessages={[]}
        convState={idleState}
        onRetry={vi.fn()}
        onOpenFile={undefined}
        conversationId="conv-lazy-find"
        systemPrompt="alpha directive"
      />,
    ));

    fireEvent.click(document.querySelector('.system-prompt-header') as HTMLElement);
    fireEvent.keyDown(window, { key: 'f', metaKey: true });
    expect(screen.getByRole('textbox', { name: 'Find in viewer' })).toHaveValue('');
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'alpha' } });
    expect(screen.getByText('1 of 1')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Close' }));
    await waitFor(() => expect(screen.queryByRole('textbox', { name: 'Find in viewer' })).toBeNull());
  });

  it('renders overlay scope without opening transcript find shortcuts', () => {
    render(withConvContext(
      <PushScopeOnMount scopeId="overlay-scope">
        <MessageList
          messages={[makeMessage(1)]}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-escape-scope"
        />
      </PushScopeOnMount>,
    ));

    fireEvent.keyDown(window, { key: 'f', metaKey: true });
    expect(screen.queryByRole('textbox', { name: 'Find in viewer' })).toBeNull();
  });

  it('renders skill rows using the same visible trigger, source, and snippet fields users see', () => {
    const skillMessage = {
      ...makeMessage(7, 'skill'),
      content: {
        name: 'dogfood',
        trigger: '/dogfood alpha --trace',
        args: 'alpha --trace',
        source: '/skills/dogfood/SKILL.md',
        snippet: 'Alpha walkthrough',
      },
    } as unknown as Message;

    render(withConvContext(
      <MessageList
        messages={[skillMessage]}
        pendingMessages={[]}
        convState={idleState}
        onRetry={vi.fn()}
        onOpenFile={undefined}
        conversationId="conv-skill-find"
      />,
    ));

    expect(screen.getByText('dogfood')).toBeInTheDocument();
    expect(screen.getByText('alpha --trace')).toBeInTheDocument();
  });

  it('renders sub-agent rows in completed-then-pending order', () => {
    const awaitingState: ConversationState = {
      type: 'awaiting_sub_agents',
      pending: [{ agent_id: 'p1', task: 'shared alpha pending' }],
      completed_results: [{ agent_id: 'c1', task: 'shared alpha completed', outcome: { type: 'success', result: 'done' } }],
    } as ConversationState;

    render(withConvContext(
      <MessageList
        messages={[]}
        pendingMessages={[]}
        convState={awaitingState}
        onRetry={vi.fn()}
        onOpenFile={undefined}
        conversationId="conv-subagent-find"
      />,
    ));

    const text = screen.getByTestId('subagent-status-mock').textContent;
    expect(text?.indexOf('completed shared alpha completed')).toBeLessThan(text?.indexOf('pending shared alpha pending') ?? -1);
  });

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

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
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

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
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

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
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

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
      ),
    );

    // buildRenderUnits consumes trailing tool messages into the owning
    // agent_turn's toolResultsByUseId map; there is exactly one agent
    // row, never standalone tool rows (REQ-MLRU-002).
    expect(container.querySelector('[data-sequence-id="21"]')).not.toBeNull();
    expect(container.querySelectorAll('.message.agent')).toHaveLength(1);
  });

  it('makes the stamped VirtualTranscript scroller the chat route scroll owner', async () => {
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

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
      ),
    );

    const mainArea = container.querySelector<HTMLElement>('#main-area');
    await waitFor(() => expect(container.querySelector('#messages')).not.toBeNull());
    const messagesScroller = container.querySelector<HTMLElement>('#messages');

    expect(mainArea).not.toBeNull();
    expect(messagesScroller).not.toBeNull();
    expect(messagesScroller).toBe(container.querySelector('[data-testid="mock-virtual-transcript"]'));
    expect(mainArea).toHaveClass('chat-main-area');
    expect(appCss).toMatch(/#main-area\s*{[^}]*overflow:\s*hidden auto;/s);
    expect(appCss).toMatch(/#main-area\.chat-main-area\s*{[^}]*overflow:\s*hidden;/s);
    expect(appCss).toMatch(/\.desktop-main\s*{[^}]*overflow:\s*auto;/s);
    expect(appCss).toMatch(/\.desktop-main:has\(\.chat-main-area\)\s*{[^}]*overflow:\s*hidden;/s);
    expect(getComputedStyle(messagesScroller!).overflowY).toBe('auto');
  });

  it('sizes wide markdown tables from the non-scrolling chat boundary', () => {
    const chatViewRule = appCss.match(/#chat-view\.view\.active\s*{[^}]*}/s)?.[0];
    const tableFallbackRule = appCss.match(/\.markdown-table-scroll\s*{[^}]*}/s)?.[0];
    const tableBreakoutRule = appCss.match(/(\.message\.agent\s*>\s*\.message-content\s*>\s*\.agent-text-block\s*>\s*\.markdown-table-scroll)\s*{([^}]*)}/s);
    const tableBreakoutTableRule = appCss.match(/\.message\.agent\s*>\s*\.message-content\s*>\s*\.agent-text-block\s*>\s*\.markdown-table-scroll\s*>\s*table\s*{([^}]*)}/s)?.[1];
    const transcriptRule = appCss.match(/\.message-virtual-transcript\s*{[^}]*}/s)?.[0];

    expect(chatViewRule).toMatch(/container-type:\s*inline-size/);
    expect(tableFallbackRule).toMatch(/max-width:\s*100%/);
    expect(tableFallbackRule).toMatch(/overflow-x:\s*auto/);
    expect(tableBreakoutRule?.[1]).toBe('.message.agent > .message-content > .agent-text-block > .markdown-table-scroll');
    expect(tableBreakoutRule?.[2]).toMatch(/width:\s*calc\(100cqw\s*-\s*16px\)/);
    expect(tableBreakoutRule?.[2]).toMatch(/margin-inline:\s*calc\(\(100%\s*-\s*\(100cqw\s*-\s*16px\)\)\s*\/\s*2\)/);
    expect(tableBreakoutRule?.[2]).not.toMatch(/transform|position|left:/);
    expect(tableBreakoutTableRule).toMatch(/min-width:\s*min\(100%,\s*784px\)/);
    expect(tableBreakoutTableRule).toMatch(/margin-inline:\s*auto/);
    expect(transcriptRule).not.toMatch(/container-type/);
    expect(transcriptRule).not.toMatch(/overflow-x/);
  });

  it('renders a 100-message conversation without throwing', () => {
    // The deleted spacer-based windowing layer had a separate test that
    // asserted a bounded number of rendered units + presence of spacers.
    // With VirtualTranscript owning virtualization, that's a library concern;
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

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
      ),
    );

    expect(container.querySelectorAll('[data-render-unit-key]').length).toBe(100);
  });

  it('uses one VirtualTranscript-owned position command per chapter jump without delayed DOM correction', () => {
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

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
        ),
      );

      const scroller = container.querySelector<HTMLElement>('#messages')!;
      scroller.scrollTop = 100;
      const firstMessage = container.querySelector<HTMLElement>('[data-render-unit-key="msg-1"] .message')!;
      const secondMessage = container.querySelector<HTMLElement>('[data-render-unit-key="msg-2"] .message')!;

      act(() => listRef.current?.scrollToUnitIndex(0));
      expect(virtualTranscriptMock.scrollToIndex).toHaveBeenLastCalledWith(0, 'start');
      expect(firstMessage).toHaveClass('jump-highlight');

      act(() => listRef.current?.scrollToUnitIndex(1));
      expect(virtualTranscriptMock.scrollToIndex).toHaveBeenLastCalledWith(1, 'start');
      expect(virtualTranscriptMock.scrollToIndex).toHaveBeenCalledTimes(2);
      expect(firstMessage).not.toHaveClass('jump-highlight');
      expect(secondMessage).toHaveClass('jump-highlight');

      act(() => vi.advanceTimersByTime(601));
      expect(scroller.scrollTop).toBe(100);
      expect(virtualTranscriptMock.scrollToIndex).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it('highlights only the newest pending jump when virtualized rows mount late', () => {
    const historical = Array.from({ length: 3 }, (_, i) => makeMessage(i + 1, 'user'));
    const listRef = createRef<React.ElementRef<typeof MessageList>>();
    virtualTranscriptMock.renderedIndices = new Set([0]);
    const renderList = () => withConvContext(
      <MessageList
        ref={listRef}
        messages={historical}
        pendingMessages={[]}
        convState={idleState}
        onRetry={vi.fn()}
        onOpenFile={undefined}
        conversationId="conv-delayed-jumps"

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
    );
    const { container, rerender } = render(renderList());
    const scroller = container.querySelector<HTMLElement>('#messages')!;
    scroller.scrollTop = 100;

    act(() => listRef.current?.scrollToUnitIndex(1));
    act(() => listRef.current?.scrollToUnitIndex(2));
    expect(virtualTranscriptMock.scrollToIndex).toHaveBeenCalledTimes(2);

    virtualTranscriptMock.renderedIndices = new Set([0, 1]);
    rerender(renderList());
    expect(container.querySelector('[data-render-unit-key="msg-2"] .message')).not.toHaveClass('jump-highlight');

    virtualTranscriptMock.renderedIndices = new Set([0, 1, 2]);
    rerender(renderList());
    expect(container.querySelector('[data-render-unit-key="msg-2"] .message')).not.toHaveClass('jump-highlight');
    expect(container.querySelector('[data-render-unit-key="msg-3"] .message')).toHaveClass('jump-highlight');
    expect(scroller.scrollTop).toBe(100);
    expect(virtualTranscriptMock.scrollToIndex).toHaveBeenCalledTimes(2);
  });

  it('does not apply a pending highlight to a new conversation reusing the same row key', () => {
    const listRef = createRef<React.ElementRef<typeof MessageList>>();
    const messages = [makeMessage(1, 'user')];
    virtualTranscriptMock.renderedIndices = new Set();
    const renderList = (conversationId: string) => withConvContext(
      <MessageList
        ref={listRef}
        messages={messages}
        pendingMessages={[]}
        convState={idleState}
        onRetry={vi.fn()}
        onOpenFile={undefined}
        conversationId={conversationId}

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
    );
    const { container, rerender } = render(renderList('conv-old'));

    act(() => listRef.current?.scrollToUnitIndex(0));
    virtualTranscriptMock.renderedIndices = new Set([0]);
    rerender(renderList('conv-new'));

    expect(container.querySelector('[data-render-unit-key="msg-1"] .message')).not.toHaveClass('jump-highlight');
  });

  // Toggling the system prompt must not change the VirtualTranscript header's
  // component *type*, only its props. A type swap forces VirtualTranscript to
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

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
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

describe('history scroll acknowledgement + continuity suppression', () => {
  it('acknowledges restore commands exactly once after geometric continuity is verified', () => {
    const messages = [makeMessage(1, 'user'), makeMessage(2, 'user'), makeMessage(3, 'user')];
    const onHistoryScrollCommandHandled = vi.fn();
    const onVisibleRangeChange = vi.fn();

    render(
      withConvContext(
        <MessageList
          messages={messages}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-history"
          transcriptPositioning={transcriptPositioningForCommand(makeRestoreAfterPrefixExpansionCommand())}
          onHistoryScrollCommandHandled={onHistoryScrollCommandHandled}
          onVisibleRangeChange={onVisibleRangeChange}
        />,
      ),
    );

    expect(virtualTranscriptMock.scrollToIndex).toHaveBeenCalledWith(1, 'start', -24);
    expect(onHistoryScrollCommandHandled).not.toHaveBeenCalled();

    act(() => virtualTranscriptMock.rangeChanged?.({ renderedRange: { startIndex: 0, endIndex: 0 }, visibleRange: { startIndex: 0, endIndex: 0 }, viewportTop: 0, layoutRevision: virtualTranscriptMock.layoutRevision() }));
    act(() => virtualTranscriptMock.rangeChanged?.({ renderedRange: { startIndex: 0, endIndex: 1 }, visibleRange: { startIndex: 0, endIndex: 0 }, viewportTop: 0, layoutRevision: virtualTranscriptMock.layoutRevision(), targetIndex: 1, targetOffset: -30 }));
    expect(onHistoryScrollCommandHandled).not.toHaveBeenCalled();

    act(() => virtualTranscriptMock.rangeChanged?.({ renderedRange: { startIndex: 1, endIndex: 2 }, visibleRange: { startIndex: 1, endIndex: 1 }, viewportTop: 0, layoutRevision: virtualTranscriptMock.layoutRevision(), targetIndex: 1, targetOffset: -22.5 }));

    expect(onHistoryScrollCommandHandled).toHaveBeenCalledTimes(1);
    expect(onHistoryScrollCommandHandled).toHaveBeenCalledWith(1, 'applied', { conversationId: 'conv-history', generation: 1, transcriptGeneration: 1 });
    expect(onVisibleRangeChange).toHaveBeenCalledTimes(3);

    act(() => virtualTranscriptMock.rangeChanged?.({ renderedRange: { startIndex: 1, endIndex: 2 }, visibleRange: { startIndex: 0, endIndex: 0 }, viewportTop: 0, layoutRevision: virtualTranscriptMock.layoutRevision() }));
    expect(onHistoryScrollCommandHandled).toHaveBeenCalledTimes(1);
  });

  it('does not acknowledge restore when the target is only overscanned, not visible', () => {
    const messages = [makeMessage(1, 'user'), makeMessage(2, 'user'), makeMessage(3, 'user')];
    const onHistoryScrollCommandHandled = vi.fn();

    render(
      withConvContext(
        <MessageList
          messages={messages}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-history"
          transcriptPositioning={transcriptPositioningForCommand(makeRestoreAfterPrefixExpansionCommand())}
          onHistoryScrollCommandHandled={onHistoryScrollCommandHandled}
        />,
      ),
    );

    act(() => virtualTranscriptMock.rangeChanged?.({
      renderedRange: { startIndex: 0, endIndex: 2 },
      visibleRange: { startIndex: 0, endIndex: 0 },
      viewportTop: 0,
      layoutRevision: virtualTranscriptMock.layoutRevision(),
      targetIndex: 1,
      targetOffset: -24,
    }));

    expect(onHistoryScrollCommandHandled).not.toHaveBeenCalled();
  });

  it('acknowledges restore commands immediately and exactly once when the target is already visible', () => {
    const messages = [makeMessage(1, 'user'), makeMessage(2, 'user'), makeMessage(3, 'user')];
    const onHistoryScrollCommandHandled = vi.fn();
    const onVisibleRangeChange = vi.fn();
    const renderList = (historyScrollCommand?: HistoryScrollCommand) => withConvContext(
      <MessageList
        messages={messages}
        pendingMessages={[]}
        convState={idleState}
        onRetry={vi.fn()}
        onOpenFile={undefined}
        conversationId="conv-history"
        transcriptPositioning={transcriptPositioningForCommand(historyScrollCommand)}
        onHistoryScrollCommandHandled={onHistoryScrollCommandHandled}
        onVisibleRangeChange={onVisibleRangeChange}
      />,
    );

    const { rerender } = render(renderList());
    act(() => virtualTranscriptMock.rangeChanged?.({ renderedRange: { startIndex: 0, endIndex: 2 }, visibleRange: { startIndex: 0, endIndex: 0 }, viewportTop: 0, layoutRevision: virtualTranscriptMock.layoutRevision() }));

    expect(onHistoryScrollCommandHandled).not.toHaveBeenCalled();
    expect(onVisibleRangeChange).toHaveBeenCalledTimes(1);

    virtualTranscriptMock.physicalSnapshot.mockImplementation((targetIndex?: number) => targetIndex === undefined
      ? virtualTranscriptMock.currentSnapshot
      : { ...virtualTranscriptMock.currentSnapshot, visibleRange: { startIndex: 1, endIndex: 1 }, targetIndex, targetOffset: -24.5 });
    rerender(renderList(makeRestoreAfterPrefixExpansionCommand()));

    expect(virtualTranscriptMock.scrollToIndex).toHaveBeenCalledWith(1, 'start', -24);
    expect(onHistoryScrollCommandHandled).toHaveBeenCalledTimes(1);
    expect(onHistoryScrollCommandHandled).toHaveBeenCalledWith(1, 'applied', { conversationId: 'conv-history', generation: 1, transcriptGeneration: 1 });

    act(() => virtualTranscriptMock.rangeChanged?.({ renderedRange: { startIndex: 0, endIndex: 2 }, visibleRange: { startIndex: 0, endIndex: 0 }, viewportTop: 0, layoutRevision: virtualTranscriptMock.layoutRevision() }));
    expect(onHistoryScrollCommandHandled).toHaveBeenCalledTimes(1);
  });

  it('suppresses continuity until user input releases it before machine interaction dispatch', () => {
    const messages = [makeMessage(1, 'user'), makeMessage(2, 'user'), makeMessage(3, 'user')];
    const onHistoryScrollCommandHandled = vi.fn();
    const onVisibleRangeChange = vi.fn();
    const { container } = render(
      withConvContext(
        <MessageList
          messages={messages}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-history"
          transcriptPositioning={transcriptPositioningForCommand(makeRestoreAfterPrefixExpansionCommand())}
          onHistoryScrollCommandHandled={onHistoryScrollCommandHandled}
          onVisibleRangeChange={onVisibleRangeChange}
        />,
      ),
    );

    const scroller = container.querySelector<HTMLElement>('#messages')!;
    fireEvent.scroll(scroller);
    expect(onVisibleRangeChange).not.toHaveBeenCalled();

    fireEvent.wheel(scroller, { deltaY: 10 });
    fireEvent.scroll(scroller);
    expect(onVisibleRangeChange).not.toHaveBeenCalled();
    expect(onHistoryScrollCommandHandled).toHaveBeenCalledWith(1, 'superseded', { conversationId: 'conv-history', generation: 1, transcriptGeneration: 1 });

    act(() => virtualTranscriptMock.rangeChanged?.({ renderedRange: { startIndex: 0, endIndex: 0 }, visibleRange: { startIndex: 0, endIndex: 0 }, viewportTop: 0, layoutRevision: virtualTranscriptMock.layoutRevision() }));
    expect(onHistoryScrollCommandHandled).toHaveBeenCalledTimes(1);

    fireEvent.scroll(scroller);
    expect(onVisibleRangeChange).toHaveBeenCalledTimes(1);
  });

  it('supersedes active restore when a suppressed scroll diverges from the desired offset', () => {
    const messages = [makeMessage(1, 'user'), makeMessage(2, 'user'), makeMessage(3, 'user')];
    const onHistoryScrollCommandHandled = vi.fn();
    const { container } = render(
      withConvContext(
        <MessageList
          messages={messages}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-history"
          transcriptPositioning={transcriptPositioningForCommand(makeRestoreAfterPrefixExpansionCommand())}
          onHistoryScrollCommandHandled={onHistoryScrollCommandHandled}
        />,
      ),
    );

    virtualTranscriptMock.physicalSnapshot.mockReturnValue({ renderedRange: { startIndex: 1, endIndex: 2 }, visibleRange: { startIndex: 0, endIndex: 0 }, viewportTop: 0, layoutRevision: 7, targetIndex: 1, targetOffset: -30 });

    fireEvent.scroll(container.querySelector<HTMLElement>('#messages')!);

    expect(onHistoryScrollCommandHandled).toHaveBeenCalledTimes(1);
    expect(onHistoryScrollCommandHandled).toHaveBeenCalledWith(1, 'superseded', { conversationId: 'conv-history', generation: 1, transcriptGeneration: 1 });
  });

  it('uses one atomic snapshot when scrollTop changes during suppressed-scroll observation', () => {
    const messages = [makeMessage(1, 'user'), makeMessage(2, 'user'), makeMessage(3, 'user')];
    const onHistoryScrollCommandHandled = vi.fn();
    const { container } = render(
      withConvContext(
        <MessageList
          messages={messages}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-history"
          transcriptPositioning={transcriptPositioningForCommand(makeRestoreAfterPrefixExpansionCommand())}
          onHistoryScrollCommandHandled={onHistoryScrollCommandHandled}
        />,
      ),
    );

    virtualTranscriptMock.physicalSnapshot.mockImplementation((targetIndex?: number) => ({
      renderedRange: { startIndex: 1, endIndex: 2 },
      visibleRange: { startIndex: 1, endIndex: 1 },
      viewportTop: 99,
      layoutRevision: 8,
      ...(targetIndex === undefined ? {} : { targetIndex, targetOffset: -30 }),
    }));

    fireEvent.scroll(container.querySelector<HTMLElement>('#messages')!);

    expect(onHistoryScrollCommandHandled).toHaveBeenCalledWith(1, 'superseded', { conversationId: 'conv-history', generation: 1, transcriptGeneration: 1 });
    expect(virtualTranscriptMock.measureOffsetForIndexAtSnapshot).not.toHaveBeenCalled();
  });

  it('does not supersede active restore when a suppressed scroll remains within desired offset tolerance', () => {
    const messages = [makeMessage(1, 'user'), makeMessage(2, 'user'), makeMessage(3, 'user')];
    const onHistoryScrollCommandHandled = vi.fn();
    const { container } = render(
      withConvContext(
        <MessageList
          messages={messages}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-history"
          transcriptPositioning={transcriptPositioningForCommand(makeRestoreAfterPrefixExpansionCommand())}
          onHistoryScrollCommandHandled={onHistoryScrollCommandHandled}
        />,
      ),
    );

    virtualTranscriptMock.physicalSnapshot.mockReturnValue({ renderedRange: { startIndex: 1, endIndex: 2 }, visibleRange: { startIndex: 0, endIndex: 0 }, viewportTop: 0, layoutRevision: 7 });
    virtualTranscriptMock.measureOffsetForIndexAtSnapshot.mockReturnValue(-22.5);

    fireEvent.scroll(container.querySelector<HTMLElement>('#messages')!);

    expect(onHistoryScrollCommandHandled).not.toHaveBeenCalled();
  });

  it('does not supersede active restore when a suppressed scroll remains within desired offset tolerance', () => {
    const messages = [makeMessage(1, 'user'), makeMessage(2, 'user'), makeMessage(3, 'user')];
    const onHistoryScrollCommandHandled = vi.fn();
    const { container } = render(
      withConvContext(
        <MessageList
          messages={messages}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-history"
          transcriptPositioning={transcriptPositioningForCommand(makeRestoreAfterPrefixExpansionCommand())}
          onHistoryScrollCommandHandled={onHistoryScrollCommandHandled}
        />,
      ),
    );

    virtualTranscriptMock.physicalSnapshot.mockReturnValue({ renderedRange: { startIndex: 1, endIndex: 2 }, visibleRange: { startIndex: 0, endIndex: 0 }, viewportTop: 0, layoutRevision: 7 });
    virtualTranscriptMock.measureOffsetForIndexAtSnapshot.mockReturnValue(-22.5);

    fireEvent.scroll(container.querySelector<HTMLElement>('#messages')!);

    expect(onHistoryScrollCommandHandled).not.toHaveBeenCalled();
  });

  it('range acknowledgement releases suppression for subsequent continuity handling', () => {
    const messages = [makeMessage(1, 'user'), makeMessage(2, 'user'), makeMessage(3, 'user')];
    const onHistoryScrollCommandHandled = vi.fn();
    const onVisibleRangeChange = vi.fn();
    const { container } = render(
      withConvContext(
        <MessageList
          messages={messages}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-history"
          transcriptPositioning={transcriptPositioningForCommand(makeRestoreAfterPrefixExpansionCommand())}
          onHistoryScrollCommandHandled={onHistoryScrollCommandHandled}
          onVisibleRangeChange={onVisibleRangeChange}
        />,
      ),
    );

    const scroller = container.querySelector<HTMLElement>('#messages')!;
    fireEvent.scroll(scroller);
    expect(onVisibleRangeChange).not.toHaveBeenCalled();

    act(() => virtualTranscriptMock.rangeChanged?.({ renderedRange: { startIndex: 1, endIndex: 2 }, visibleRange: { startIndex: 1, endIndex: 1 }, viewportTop: 0, layoutRevision: virtualTranscriptMock.layoutRevision(), targetIndex: 1, targetOffset: -24 }));
    expect(onHistoryScrollCommandHandled).toHaveBeenCalledTimes(1);

    fireEvent.scroll(scroller);
    expect(onVisibleRangeChange).toHaveBeenCalledTimes(1);
  });

  it('does not acknowledge continuity from range alone when measured geometry is outside tolerance', () => {
    const messages = [makeMessage(1, 'user'), makeMessage(2, 'user'), makeMessage(3, 'user')];
    const onHistoryScrollCommandHandled = vi.fn();
    render(
      withConvContext(
        <MessageList
          messages={messages}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-history"
          transcriptPositioning={transcriptPositioningForCommand(makeRestoreAfterPrefixExpansionCommand())}
          onHistoryScrollCommandHandled={onHistoryScrollCommandHandled}
        />,
      ),
    );

    virtualTranscriptMock.measureOffsetForIndex.mockReturnValue(-21.5);
    act(() => virtualTranscriptMock.rangeChanged?.({ renderedRange: { startIndex: 1, endIndex: 2 }, visibleRange: { startIndex: 0, endIndex: 0 }, viewportTop: 0, layoutRevision: virtualTranscriptMock.layoutRevision() }));
    expect(onHistoryScrollCommandHandled).not.toHaveBeenCalled();
  });

  it('clears suppressed continuity when a newer restore command supersedes the old one', () => {
    const messages = [makeMessage(1, 'user'), makeMessage(2, 'user'), makeMessage(3, 'user')];
    const onHistoryScrollCommandHandled = vi.fn();
    const onVisibleRangeChange = vi.fn();
    const currentHistoryView = { conversationId: 'conv-history', generation: 1, transcriptGeneration: 1 };
    const renderList = (historyScrollCommand: HistoryScrollCommand) => withConvContext(
      <MessageList
        messages={messages}
        pendingMessages={[]}
        convState={idleState}
        onRetry={vi.fn()}
        onOpenFile={undefined}
        conversationId="conv-history"
        transcriptPositioning={transcriptPositioningForCommand(historyScrollCommand)}
        onHistoryScrollCommandHandled={onHistoryScrollCommandHandled}
        onVisibleRangeChange={onVisibleRangeChange}
      />,
    );

    const { container, rerender } = render(renderList(makeRestoreAfterPrefixExpansionCommand({ token: 1, messageId: 'msg-2', view: currentHistoryView })));
    const scroller = container.querySelector<HTMLElement>('#messages')!;

    fireEvent.scroll(scroller);
    expect(onVisibleRangeChange).not.toHaveBeenCalled();

    rerender(renderList(makeRestoreAfterPrefixExpansionCommand({ token: 2, messageId: 'msg-3', view: currentHistoryView })));
    fireEvent.scroll(scroller);
    expect(onVisibleRangeChange).not.toHaveBeenCalled();

    act(() => virtualTranscriptMock.rangeChanged?.({ renderedRange: { startIndex: 2, endIndex: 2 }, visibleRange: { startIndex: 2, endIndex: 2 }, viewportTop: 0, layoutRevision: virtualTranscriptMock.layoutRevision(), targetIndex: 2, targetOffset: -24 }));
    expect(onHistoryScrollCommandHandled).toHaveBeenCalledTimes(2);
    expect(onHistoryScrollCommandHandled).toHaveBeenCalledWith(1, 'superseded', { conversationId: 'conv-history', generation: 1, transcriptGeneration: 1 });
    expect(onHistoryScrollCommandHandled).toHaveBeenCalledWith(2, 'applied', { conversationId: 'conv-history', generation: 1, transcriptGeneration: 1 });
    expect(virtualTranscriptMock.scrollToIndex).toHaveBeenLastCalledWith(2, 'start', -24);
  });

  it('supersedes an active restore when its command disappears', () => {
    const view = { conversationId: 'conv-history', generation: 1, transcriptGeneration: 1 };
    const command = makeRestoreAfterPrefixExpansionCommand({ token: 1, view });
    const onHistoryScrollCommandHandled = vi.fn();
    const renderList = (historyScrollCommand: HistoryScrollCommand | null) => withConvContext(
      <MessageList
        messages={[makeMessage(1, 'user'), makeMessage(2, 'user')]}
        pendingMessages={[]}
        convState={idleState}
        onRetry={vi.fn()}
        onOpenFile={undefined}
        conversationId="conv-history"
        transcriptPositioning={transcriptPositioningForCommand(historyScrollCommand)}
        onHistoryScrollCommandHandled={onHistoryScrollCommandHandled}
      />,
    );

    const { rerender } = render(renderList(command));
    expect(virtualTranscriptMock.scrollToIndex).toHaveBeenCalledTimes(1);

    rerender(renderList(null));
    expect(onHistoryScrollCommandHandled).toHaveBeenCalledTimes(1);
    expect(onHistoryScrollCommandHandled).toHaveBeenCalledWith(1, 'superseded', view);
  });

  it('does not supersede the active owner on React effect cleanup', () => {
    const view = { conversationId: 'conv-history', generation: 1, transcriptGeneration: 1 };
    const command = makeRestoreAfterPrefixExpansionCommand({ token: 1, view });
    const onHistoryScrollCommandHandled = vi.fn();

    const { unmount } = render(withConvContext(
      <MessageList
        messages={[makeMessage(1, 'user'), makeMessage(2, 'user')]}
        pendingMessages={[]}
        convState={idleState}
        onRetry={vi.fn()}
        onOpenFile={undefined}
        conversationId="conv-history"
        transcriptPositioning={transcriptPositioningForCommand(command)}
        onHistoryScrollCommandHandled={onHistoryScrollCommandHandled}
      />,
    ));
    expect(virtualTranscriptMock.scrollToIndex).toHaveBeenCalledTimes(1);

    unmount();
    unmount();

    expect(onHistoryScrollCommandHandled).not.toHaveBeenCalled();
  });

  it('does not let StrictMode setup-cleanup-setup terminally supersede a mounted command', () => {
    const view = { conversationId: 'conv-history', generation: 1, transcriptGeneration: 1 };
    const command = makeRestoreAfterPrefixExpansionCommand({ token: 1, view });
    const onHistoryScrollCommandHandled = vi.fn();

    render(withConvContext(
      <StrictMode>
        <MessageList
          messages={[makeMessage(1, 'user'), makeMessage(2, 'user')]}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-history"
          transcriptPositioning={transcriptPositioningForCommand(command)}
          onHistoryScrollCommandHandled={onHistoryScrollCommandHandled}
        />
      </StrictMode>,
    ));

    act(() => virtualTranscriptMock.rangeChanged?.({ renderedRange: { startIndex: 1, endIndex: 1 }, visibleRange: { startIndex: 1, endIndex: 1 }, viewportTop: 0, layoutRevision: virtualTranscriptMock.layoutRevision(), targetIndex: 1, targetOffset: -24 }));

    expect(onHistoryScrollCommandHandled).toHaveBeenCalledTimes(1);
    expect(onHistoryScrollCommandHandled).toHaveBeenCalledWith(1, 'applied', view);
  });

  it('supersedes a pending jump exactly once when a newer jump replaces it', () => {
    const messages = [makeMessage(1, 'user'), makeMessage(2, 'user'), makeMessage(3, 'user')];
    const onHistoryScrollCommandHandled = vi.fn();
    const view = { conversationId: 'conv-history', generation: 1, transcriptGeneration: 1 };
    const renderList = (command: HistoryScrollCommand) => withConvContext(
      <MessageList
        messages={messages}
        pendingMessages={[]}
        convState={idleState}
        onRetry={vi.fn()}
        onOpenFile={undefined}
        conversationId="conv-history"
        transcriptPositioning={transcriptPositioningForCommand(command)}
        onHistoryScrollCommandHandled={onHistoryScrollCommandHandled}
      />,
    );

    const { rerender } = render(renderList(makeJumpToMessageCommand({ token: 1, targetMessageId: 'msg-2', view })));
    expect(virtualTranscriptMock.scrollToIndex).toHaveBeenCalledWith(1, 'start');

    rerender(renderList(makeJumpToMessageCommand({ token: 2, targetMessageId: 'msg-3', view })));
    expect(onHistoryScrollCommandHandled).toHaveBeenCalledTimes(1);
    expect(onHistoryScrollCommandHandled).toHaveBeenCalledWith(1, 'superseded', view);
    expect(virtualTranscriptMock.scrollToIndex).toHaveBeenLastCalledWith(2, 'start');

    rerender(renderList(makeJumpToMessageCommand({ token: 2, targetMessageId: 'msg-3', view })));
    expect(onHistoryScrollCommandHandled).toHaveBeenCalledTimes(1);
  });

  it('supersedes a pending jump exactly once when its command disappears', () => {
    const view = { conversationId: 'conv-history', generation: 1, transcriptGeneration: 1 };
    const command = makeJumpToMessageCommand({ token: 1, targetMessageId: 'msg-2', view });
    const onHistoryScrollCommandHandled = vi.fn();
    const renderList = (historyScrollCommand: HistoryScrollCommand | null) => withConvContext(
      <MessageList
        messages={[makeMessage(1, 'user'), makeMessage(2, 'user')]}
        pendingMessages={[]}
        convState={idleState}
        onRetry={vi.fn()}
        onOpenFile={undefined}
        conversationId="conv-history"
        transcriptPositioning={transcriptPositioningForCommand(historyScrollCommand)}
        onHistoryScrollCommandHandled={onHistoryScrollCommandHandled}
      />,
    );

    const { rerender } = render(renderList(command));
    expect(virtualTranscriptMock.scrollToIndex).toHaveBeenCalledTimes(1);

    rerender(renderList(null));
    rerender(renderList(null));
    expect(onHistoryScrollCommandHandled).toHaveBeenCalledTimes(1);
    expect(onHistoryScrollCommandHandled).toHaveBeenCalledWith(1, 'superseded', view);
  });

  it('supersedes a pending jump exactly once on user interaction', () => {
    const view = { conversationId: 'conv-history', generation: 1, transcriptGeneration: 1 };
    const onHistoryScrollCommandHandled = vi.fn();
    const { container } = render(
      withConvContext(
        <MessageList
          messages={[makeMessage(1, 'user'), makeMessage(2, 'user')]}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-history"
          transcriptPositioning={transcriptPositioningForCommand(makeJumpToMessageCommand({ token: 1, targetMessageId: 'msg-2', view }))}
          onHistoryScrollCommandHandled={onHistoryScrollCommandHandled}
        />,
      ),
    );

    const scroller = container.querySelector<HTMLElement>('#messages')!;
    fireEvent.wheel(scroller, { deltaY: 10 });
    fireEvent.pointerDown(scroller);

    expect(onHistoryScrollCommandHandled).toHaveBeenCalledTimes(1);
    expect(onHistoryScrollCommandHandled).toHaveBeenCalledWith(1, 'superseded', view);
  });

  it('clears suppressed continuity on conversation change', () => {
    const messages = [makeMessage(1, 'user'), makeMessage(2, 'user'), makeMessage(3, 'user')];
    const onHistoryScrollCommandHandled = vi.fn();
    const onVisibleRangeChange = vi.fn();
    const renderList = (conversationId: string, token: number) => withConvContext(
      <MessageList
        messages={messages}
        pendingMessages={[]}
        convState={idleState}
        onRetry={vi.fn()}
        onOpenFile={undefined}
        conversationId={conversationId}
        transcriptPositioning={transcriptPositioningForCommand(makeRestoreAfterPrefixExpansionCommand({ token, view: { conversationId, generation: 1, transcriptGeneration: 1 } }))}
        onHistoryScrollCommandHandled={onHistoryScrollCommandHandled}
        onVisibleRangeChange={onVisibleRangeChange}
      />,
    );

    const { container, rerender } = render(renderList('conv-history-a', 1));
    const scroller = container.querySelector<HTMLElement>('#messages')!;
    fireEvent.scroll(scroller);
    expect(onVisibleRangeChange).not.toHaveBeenCalled();

    rerender(renderList('conv-history-b', 2));
    fireEvent.scroll(scroller);
    expect(onVisibleRangeChange).not.toHaveBeenCalled();
    expect(onHistoryScrollCommandHandled).toHaveBeenCalledTimes(1);
    expect(onHistoryScrollCommandHandled).toHaveBeenCalledWith(
      1,
      'superseded',
      { conversationId: 'conv-history-a', generation: 1, transcriptGeneration: 1 },
    );
  });

  it('does not let a stale history range callback acknowledge a new conversation with overlapping indices', () => {
    const messages = [makeMessage(1, 'user'), makeMessage(2, 'user'), makeMessage(3, 'user')];
    const onHistoryScrollCommandHandled = vi.fn();
    const renderList = (conversationId: string, token: number) => withConvContext(
      <MessageList
        messages={messages}
        pendingMessages={[]}
        convState={idleState}
        onRetry={vi.fn()}
        onOpenFile={undefined}
        conversationId={conversationId}
        transcriptPositioning={transcriptPositioningForCommand(makeRestoreAfterPrefixExpansionCommand({ token, view: { conversationId, generation: 1, transcriptGeneration: 1 } }))}
        onHistoryScrollCommandHandled={onHistoryScrollCommandHandled}
      />,
    );

    const { rerender } = render(renderList('conv-history-a', 1));
    virtualTranscriptMock.measureOffsetForIndex.mockReturnValue(-21.5);
    act(() => virtualTranscriptMock.rangeChanged?.({ renderedRange: { startIndex: 1, endIndex: 2 }, visibleRange: { startIndex: 0, endIndex: 0 }, viewportTop: 0, layoutRevision: virtualTranscriptMock.layoutRevision() }));
    expect(onHistoryScrollCommandHandled).not.toHaveBeenCalled();

    rerender(renderList('conv-history-b', 2));
    act(() => virtualTranscriptMock.rangeChanged?.({ renderedRange: { startIndex: 1, endIndex: 2 }, visibleRange: { startIndex: 0, endIndex: 0 }, viewportTop: 0, layoutRevision: virtualTranscriptMock.layoutRevision() }));

    expect(onHistoryScrollCommandHandled).not.toHaveBeenCalledWith(
      1,
      'applied',
      { conversationId: 'conv-history-a', generation: 1, transcriptGeneration: 1 },
    );
  });

  it('keeps suppressed continuity local to the unmounted lifecycle', () => {
    const messages = [makeMessage(1, 'user'), makeMessage(2, 'user'), makeMessage(3, 'user')];
    const onHistoryScrollCommandHandled = vi.fn();
    const onVisibleRangeChange = vi.fn();
    const { container, unmount } = render(
      withConvContext(
        <MessageList
          messages={messages}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-history"
          transcriptPositioning={transcriptPositioningForCommand(makeRestoreAfterPrefixExpansionCommand())}
          onHistoryScrollCommandHandled={onHistoryScrollCommandHandled}
          onVisibleRangeChange={onVisibleRangeChange}
        />,
      ),
    );

    const scroller = container.querySelector<HTMLElement>('#messages')!;
    fireEvent.scroll(scroller);
    expect(onVisibleRangeChange).not.toHaveBeenCalled();

    unmount();
    expect(onHistoryScrollCommandHandled).not.toHaveBeenCalled();
  });
});

describe('history expansion feedback', () => {
  it('renders deep-link failures after coverage becomes complete', () => {
    render(
      withConvContext(
        <MessageList
          messages={[makeMessage(1, 'user')]}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-history"
          hasOlderMessages={false}
          olderHistoryError="Message target-message was not found"

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
      ),
    );

    expect(screen.getByRole('alert')).toHaveTextContent(
      'Could not load earlier history: Message target-message was not found',
    );
    expect(screen.queryByRole('button', { name: /earlier history/i })).not.toBeInTheDocument();
  });
});

describe('render-unit identity across state ticks', () => {
  it('does not re-render mounted agent rows on a conversation state tick', () => {
    // Same store and same prop identities across rerenders — only convState
    // changes, exactly like a state tick arriving over SSE.
    const store = new ConversationStore();
    const wrap = (el: React.ReactElement) => (
      <ConversationContext.Provider value={store}>{el}</ConversationContext.Provider>
    );
    const messages = [
      makeMessage(1, 'user'),
      makeMessage(2, 'agent'),
      makeMessage(3, 'agent'),
    ];
    const pending: never[] = [];
    const onRetry = vi.fn();

    const { rerender } = render(
      wrap(
        <MessageList
          messages={messages}
          pendingMessages={pending}
          convState={idleState}
          onRetry={onRetry}
          onOpenFile={undefined}
          conversationId="conv-memo-identity"

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
      ),
    );
    const mountCount = agentRenderCounter.count;
    expect(mountCount).toBeGreaterThan(0);

    // State tick: idle -> awaiting_llm. No message changed, no active tool
    // changed — a rebuild of historical units here would hand every
    // AgentMessage a fresh toolResultsByUseId Map and defeat its memo.
    rerender(
      wrap(
        <MessageList
          messages={messages}
          pendingMessages={pending}
          convState={{ type: 'awaiting_llm' }}
          onRetry={onRetry}
          onOpenFile={undefined}
          conversationId="conv-memo-identity"

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
      ),
    );
    expect(agentRenderCounter.count).toBe(mountCount);

    // Sanity check that the counter is live: an actual message change does
    // re-render agent rows.
    rerender(
      wrap(
        <MessageList
          messages={[...messages, makeMessage(4, 'agent')]}
          pendingMessages={pending}
          convState={{ type: 'awaiting_llm' }}
          onRetry={onRetry}
          onOpenFile={undefined}
          conversationId="conv-memo-identity"

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
      ),
    );
    expect(agentRenderCounter.count).toBeGreaterThan(mountCount);
  });
});

// Tests for the manual auto-follow callback (handleTotalListHeightChanged).
// VirtualTranscript is mocked as a passthrough, so these tests call the captured
// `totalExtentChanged` callback directly and assert whether the shared
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

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
      ),
    );

    const scroller = container.querySelector<HTMLElement>('#messages')!;
    // Simulate: user at bottom, height grows from 500 to 600
    setupScroller(scroller, { scrollHeight: 600, scrollTop: 100, clientHeight: 500 });
    // First call seeds the baseline via the conversation-switch handler
    // (lastSeenConvIdRef starts undefined) — no snap on initial mount
    act(() => virtualTranscriptMock.totalExtentChanged?.(500));
    // Engage (downward wheel) so the assertion exercises the pin branch,
    // not the pre-engagement settle rescue.
    fireEvent.wheel(scroller, { deltaY: 50 });
    // Clear so we only observe the re-snap
    virtualTranscriptMock.scrollToTail.mockClear();
    // Second call: height grew, user was at bottom (oldFromBottom = 500 - 100 - 500 = -100 <= 100)
    setupScroller(scroller, { scrollHeight: 600, scrollTop: 100, clientHeight: 500 });
    act(() => virtualTranscriptMock.totalExtentChanged?.(600));

    expect(virtualTranscriptMock.scrollToTail).toHaveBeenCalled();
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

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
      ),
    );

    const scroller = container.querySelector<HTMLElement>('#messages')!;
    // Seed baseline via the conversation-switch handler (no snap on mount)
    setupScroller(scroller, { scrollHeight: 1000, scrollTop: 0, clientHeight: 400 });
    act(() => virtualTranscriptMock.totalExtentChanged?.(1000));
    // The user got scrolled-up by scrolling — engagement releases the
    // pre-engagement settle rescue (which re-snaps unconditionally).
    fireEvent.wheel(scroller, { deltaY: -50 });
    // Clear so we only observe subsequent calls
    virtualTranscriptMock.scrollToTail.mockClear();

    // User scrolled up: scrollTop = 0, but content is tall (prevHeight = 1000)
    // oldFromBottom = 1000 - 0 - 400 = 600 > 100 — well past the threshold.
    // Height grows further, but user is scrolled up, so no re-snap.
    setupScroller(scroller, { scrollHeight: 1200, scrollTop: 0, clientHeight: 400 });
    act(() => virtualTranscriptMock.totalExtentChanged?.(1200));

    expect(virtualTranscriptMock.scrollToTail).not.toHaveBeenCalled();
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

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
      ),
    );

    // No content yet — callback should not snap
    const scroller = container.querySelector<HTMLElement>('#messages')!;
    setupScroller(scroller, { scrollHeight: 0, scrollTop: 0, clientHeight: 500 });
    act(() => virtualTranscriptMock.totalExtentChanged?.(0));
    expect(virtualTranscriptMock.scrollToTail).not.toHaveBeenCalled();

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

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
      ),
    );

    // First non-empty height measurement — should snap
    setupScroller(scroller, { scrollHeight: 600, scrollTop: 0, clientHeight: 500 });
    act(() => virtualTranscriptMock.totalExtentChanged?.(600));
    expect(virtualTranscriptMock.scrollToTail).toHaveBeenCalled();
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

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
      ),
    );

    // Seed baseline for conversation A.
    // The first measurement goes through the conversation-switch handler
    // (lastSeenConvIdRef starts undefined), which seeds the baseline
    // WITHOUT scrolling — initialTopMostItemIndex already placed the
    // viewport for a conversation that mounted with messages.
    const scroller = container.querySelector<HTMLElement>('#messages')!;
    setupScroller(scroller, { scrollHeight: 500, scrollTop: 100, clientHeight: 400 });
    act(() => virtualTranscriptMock.totalExtentChanged?.(500));
    expect(virtualTranscriptMock.scrollToTail).not.toHaveBeenCalled();

    // Clear mock to track new calls
    virtualTranscriptMock.scrollToTail.mockClear();

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

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
      ),
    );

    // VirtualTranscript re-keys on the conversationId change: the mock mounts a
    // FRESH scroller element, so re-query — the old `scroller` handle is
    // detached and mutating it would not affect what the component reads.
    const scrollerB = container.querySelector<HTMLElement>('#messages')!;
    setupScroller(scrollerB, { scrollHeight: 1000, scrollTop: 0, clientHeight: 400 });
    act(() => virtualTranscriptMock.totalExtentChanged?.(1000));
    // The conversation-switch handler seeds the baseline; should NOT snap
    // because hasSeenContentRef is seeded true (B already has messages)
    expect(virtualTranscriptMock.scrollToTail).not.toHaveBeenCalled();
    // The user scrolled up in B — engagement releases the settle rescue.
    fireEvent.wheel(scrollerB, { deltaY: -50 });

    // Now a delayed height delta arrives (e.g. code highlighter mount)
    // while the user is scrolled up in conversation B.
    // oldFromBottom = 1000 - 0 - 400 = 600 > 100
    setupScroller(scrollerB, { scrollHeight: 1100, scrollTop: 0, clientHeight: 400 });
    act(() => virtualTranscriptMock.totalExtentChanged?.(1100));
    // Should NOT snap — user is scrolled up, this is not a first-content case
    expect(virtualTranscriptMock.scrollToTail).not.toHaveBeenCalled();
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

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
      ),
    );

    const scroller = container.querySelector<HTMLElement>('#messages')!;
    // Mount stranding: VirtualTranscript's initial tail placement was computed
    // against early estimates; a huge correction lands and the viewport is
    // left at the top. The user has not interacted yet.
    setupScroller(scroller, { scrollHeight: 48000, scrollTop: 0, clientHeight: 600 });
    act(() => virtualTranscriptMock.totalExtentChanged?.(48000));

    // The settle snap writes scrollTop directly (a DOM snap cannot be
    // aborted by VirtualTranscript's measurement loop, unlike scrollToIndex). Record it.
    const written: number[] = [];
    Object.defineProperty(scroller, 'scrollHeight', { configurable: true, get: () => 12000000 });
    Object.defineProperty(scroller, 'scrollTop', {
      configurable: true,
      get: () => 0,
      set: (v: number) => written.push(v),
    });
    Object.defineProperty(scroller, 'clientHeight', { configurable: true, get: () => 600 });
    act(() => virtualTranscriptMock.totalExtentChanged?.(12000000));
    // The snap is deferred one frame so VirtualTranscript's own compensation for
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

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
        ),
      );

      const scroller = container.querySelector<HTMLElement>('#messages')!;
      // Silent stranding: after the first (seeding) measurement, VirtualTranscript's
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
      act(() => virtualTranscriptMock.totalExtentChanged?.(12000000));

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

  it('does not fight scroll-only inputs (keyboard, find-in-page) after the settle window', () => {
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
            conversationId="conv-scroll-only"

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
        ),
      );

      const scroller = container.querySelector<HTMLElement>('#messages')!;
      // Pinned mount; the seeding measurement starts the settle window.
      setupScroller(scroller, { scrollHeight: 1000, scrollTop: 600, clientHeight: 400 });
      act(() => virtualTranscriptMock.totalExtentChanged?.(1000));

      // Settle window (3s) elapses without any user engagement.
      act(() => vi.advanceTimersByTime(3500));
      virtualTranscriptMock.scrollToTail.mockClear();

      // Scroll-only input: browser find-in-page (or PageUp on a focused
      // row) jumps the viewport up. Emits ONLY a scroll event — no
      // touch/wheel/pointer, so engagement is never marked.
      const written: number[] = [];
      Object.defineProperty(scroller, 'scrollHeight', { configurable: true, get: () => 1000 });
      Object.defineProperty(scroller, 'scrollTop', {
        configurable: true,
        get: () => 100,
        set: (v: number) => written.push(v),
      });
      Object.defineProperty(scroller, 'clientHeight', { configurable: true, get: () => 400 });
      fireEvent.scroll(scroller);

      // A later height delta (image load, highlighter) must go through the
      // normal distance-based pin logic — the user is 500px up, so no snap
      // and no settle write.
      Object.defineProperty(scroller, 'scrollHeight', { configurable: true, get: () => 1100 });
      act(() => virtualTranscriptMock.totalExtentChanged?.(1100));
      act(() => vi.advanceTimersByTime(500));

      expect(written).toHaveLength(0);
      expect(virtualTranscriptMock.scrollToTail).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it.each([
    ['moved touch cancellation', (scroller: HTMLElement, rerender: (ui: React.ReactElement) => void) => {
      void rerender;
      fireEvent.touchStart(scroller, { touches: [{}] });
      fireEvent.touchMove(scroller, { touches: [{}] });
      fireEvent.touchCancel(scroller, { touches: [] });
    }],
    ['conversation reset', (_scroller: HTMLElement, rerender: (ui: React.ReactElement) => void) => {
      rerender(withConvContext(
        <MessageList
          messages={Array.from({ length: 5 }, (_, i) => makeMessage(i + 1, 'user'))}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-after-deferred-follow"

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
      ));
    }],
  ])('revalidates deferred tail follow after %s', async (_name, transferOwnership) => {
    let releaseFrame: FrameRequestCallback | null = null;
    const requestAnimationFrameSpy = vi
      .spyOn(window, 'requestAnimationFrame')
      .mockImplementation((callback) => {
        releaseFrame = callback;
        return 42;
      });
    const cancelAnimationFrameSpy = vi.spyOn(window, 'cancelAnimationFrame').mockImplementation(() => {});
    try {
      const historical = Array.from({ length: 5 }, (_, i) => makeMessage(i + 1, 'user'));
      const { container, rerender } = render(
        withConvContext(
          <MessageList
            messages={historical}
            pendingMessages={[]}
            convState={idleState}
            onRetry={vi.fn()}
            onOpenFile={undefined}
            conversationId="conv-deferred-follow"

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
        ),
      );
      const scroller = container.querySelector<HTMLElement>('#messages')!;
      setupScroller(scroller, { scrollHeight: 500, scrollTop: 100, clientHeight: 400 });
      act(() => virtualTranscriptMock.totalExtentChanged?.(500));
      fireEvent.pointerDown(scroller);
      releaseFrame = null;
      virtualTranscriptMock.scrollToTail.mockClear();

      rerender(withConvContext(
        <MessageList
          messages={[...historical, makeMessage(6, 'user')]}
          pendingMessages={[]}
          convState={idleState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-deferred-follow"

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
      ));
      await waitFor(() => expect(releaseFrame).not.toBeNull());

      transferOwnership(scroller, rerender);
      act(() => releaseFrame?.(performance.now()));

      expect(virtualTranscriptMock.scrollToTail).not.toHaveBeenCalled();
    } finally {
      requestAnimationFrameSpy.mockRestore();
      cancelAnimationFrameSpy.mockRestore();
    }
  });

  it('marks unread tail content when a gesture suppresses the pinned snap during tail growth', () => {
    const historical = Array.from({ length: 5 }, (_, i) => makeMessage(i + 1, 'user'));
    const subAgentsState: ConversationState = {
      type: 'awaiting_sub_agents',
      pending: [],
      completed_results: [],
    };
    const { container } = render(
      withConvContext(
        <MessageList
          messages={historical}
          pendingMessages={[]}
          convState={subAgentsState}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-suppressed-unread"

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
      ),
    );

    const scroller = container.querySelector<HTMLElement>('#messages')!;
    setupScroller(scroller, { scrollHeight: 500, scrollTop: 100, clientHeight: 400 });
    act(() => virtualTranscriptMock.totalExtentChanged?.(500));
    virtualTranscriptMock.scrollToTail.mockClear();

    // The user starts dragging up (still within the pin threshold) and
    // VirtualTranscript reports them off the bottom.
    act(() => virtualTranscriptMock.pinnedChanged?.(false));
    fireEvent.touchStart(scroller, { touches: [{}] });
    fireEvent.touchMove(scroller, { touches: [{}] });

    // Genuine tail growth (sub-agents phase) lands while the gesture
    // suppresses the snap. oldFromBottom = 500 - 100 - 400 = 0 (pinned).
    setupScroller(scroller, { scrollHeight: 600, scrollTop: 100, clientHeight: 400 });
    act(() => virtualTranscriptMock.totalExtentChanged?.(600));

    // The snap was suppressed, but the unread signal must not be swallowed:
    // this may be the last growth event before the phase ends.
    expect(virtualTranscriptMock.scrollToTail).not.toHaveBeenCalled();
    expect(container.querySelector('.jump-to-newest')).not.toBeNull();
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

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
      ),
    );

    const scroller = container.querySelector<HTMLElement>('#messages')!;
    setupScroller(scroller, { scrollHeight: 1000, scrollTop: 0, clientHeight: 400 });
    act(() => virtualTranscriptMock.totalExtentChanged?.(1000));
    // Any pointer interaction with the list counts as engagement.
    fireEvent.pointerDown(scroller);
    virtualTranscriptMock.scrollToTail.mockClear();

    // Pointer interaction exits mount rescue into normal durable follow.
    // It does not claim reading ownership without upward movement.
    setupScroller(scroller, { scrollHeight: 1200, scrollTop: 0, clientHeight: 400 });
    act(() => virtualTranscriptMock.totalExtentChanged?.(1200));
    expect(virtualTranscriptMock.scrollToTail).toHaveBeenCalled();
  });

  it('follows a pinned user even when VirtualTranscript model height disagrees with DOM scrollHeight', () => {
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

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
      ),
    );

    const scroller = container.querySelector<HTMLElement>('#messages')!;
    // User is 32px from the DOM bottom (600 - 168 - 400) — pinned. But
    // VirtualTranscript's model total is 675: a +75 estimate bias, as accumulates on
    // long conversations with many unmeasured rows. A model-based pin check
    // computes 675 - 168 - 400 = 107 > 100 and wrongly drops auto-follow.
    setupScroller(scroller, { scrollHeight: 600, scrollTop: 168, clientHeight: 400 });
    act(() => virtualTranscriptMock.totalExtentChanged?.(675));
    // Engage (downward wheel: no upward-suppression armed) so the assertion
    // exercises the distance-based pin branch, not the settle rescue.
    fireEvent.wheel(scroller, { deltaY: 50 });
    virtualTranscriptMock.scrollToTail.mockClear();

    // Tail content arrives: model 675 -> 775, DOM 600 -> 700.
    setupScroller(scroller, { scrollHeight: 700, scrollTop: 168, clientHeight: 400 });
    act(() => virtualTranscriptMock.totalExtentChanged?.(775));

    // DOM-units pin check: 600 - 168 - 400 = 32 <= 100 — followed.
    expect(virtualTranscriptMock.scrollToTail).toHaveBeenCalled();
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

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
        ),
      );

      const scroller = container.querySelector<HTMLElement>('#messages')!;
      // Seed baseline: user pinned at bottom
      setupScroller(scroller, { scrollHeight: 500, scrollTop: 100, clientHeight: 400 });
      act(() => virtualTranscriptMock.totalExtentChanged?.(500));
      virtualTranscriptMock.scrollToTail.mockClear();

      // Finger goes down and starts dragging up — still within the pin
      // threshold (oldFromBottom = 500 - 80 - 400 = 20) when a
      // measurement-driven height delta lands.
      fireEvent.touchStart(scroller, { touches: [{}] });
      fireEvent.touchMove(scroller, { touches: [{}] });
      setupScroller(scroller, { scrollHeight: 600, scrollTop: 80, clientHeight: 400 });
      act(() => virtualTranscriptMock.totalExtentChanged?.(600));
      expect(virtualTranscriptMock.scrollToTail).not.toHaveBeenCalled();

      // Finger lift and elapsed time do not release durable reading ownership.
      fireEvent.touchEnd(scroller, { touches: [] });
      act(() => vi.advanceTimersByTime(1300));
      setupScroller(scroller, { scrollHeight: 700, scrollTop: 200, clientHeight: 400 });
      act(() => virtualTranscriptMock.totalExtentChanged?.(700));
      expect(virtualTranscriptMock.scrollToTail).not.toHaveBeenCalled();
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

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
        ),
      );

      const scroller = container.querySelector<HTMLElement>('#messages')!;
      setupScroller(scroller, { scrollHeight: 500, scrollTop: 100, clientHeight: 400 });
      // Establish the scroll-direction baseline at scrollTop=100 (the
      // detector compares against the last observed scrollTop).
      fireEvent.scroll(scroller);
      act(() => virtualTranscriptMock.totalExtentChanged?.(500));
      virtualTranscriptMock.scrollToTail.mockClear();

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
      act(() => virtualTranscriptMock.totalExtentChanged?.(600));
      expect(virtualTranscriptMock.scrollToTail).not.toHaveBeenCalled();

      // Elapsed time cannot reclaim the viewport from the user.
      act(() => vi.advanceTimersByTime(1300));
      setupScroller(scroller, { scrollHeight: 700, scrollTop: 200, clientHeight: 400 });
      act(() => virtualTranscriptMock.totalExtentChanged?.(700));
      expect(virtualTranscriptMock.scrollToTail).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it('keeps pinned auto-follow after a tap-only touch during active tail growth', () => {
    vi.useFakeTimers();
    try {
      const historical = Array.from({ length: 5 }, (_, i) => makeMessage(i + 1, 'user'));
      const subAgentsState: ConversationState = {
        type: 'awaiting_sub_agents',
        pending: [],
        completed_results: [],
      };
      const { container } = render(
        withConvContext(
          <MessageList
            messages={historical}
            pendingMessages={[]}
            convState={subAgentsState}
            onRetry={vi.fn()}
            onOpenFile={undefined}
            conversationId="conv-pinned-tap"

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
        ),
      );

      const scroller = container.querySelector<HTMLElement>('#messages')!;
      vi.setSystemTime(1000);
      setupScroller(scroller, { scrollHeight: 500, scrollTop: 100, clientHeight: 400 });
      fireEvent.scroll(scroller);
      act(() => virtualTranscriptMock.totalExtentChanged?.(500));
      virtualTranscriptMock.scrollToTail.mockClear();

      vi.setSystemTime(1050);
      fireEvent.touchStart(scroller, { touches: [{}] });
      vi.setSystemTime(1060);
      fireEvent.touchEnd(scroller, { touches: [] });
      vi.setSystemTime(1100);
      setupScroller(scroller, { scrollHeight: 600, scrollTop: 100, clientHeight: 400 });
      act(() => virtualTranscriptMock.totalExtentChanged?.(600));

      expect(virtualTranscriptMock.scrollToTail).toHaveBeenCalled();
      expect(container.querySelector('.jump-to-newest')).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it('keeps pinned auto-follow after upward scroll returns to bottom before a tap', () => {
    vi.useFakeTimers();
    try {
      const historical = Array.from({ length: 5 }, (_, i) => makeMessage(i + 1, 'user'));
      const subAgentsState: ConversationState = {
        type: 'awaiting_sub_agents',
        pending: [],
        completed_results: [],
      };
      const { container } = render(
        withConvContext(
          <MessageList
            messages={historical}
            pendingMessages={[]}
            convState={subAgentsState}
            onRetry={vi.fn()}
            onOpenFile={undefined}
            conversationId="conv-return-bottom-tap"

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
        ),
      );

      const scroller = container.querySelector<HTMLElement>('#messages')!;
      vi.setSystemTime(1000);
      setupScroller(scroller, { scrollHeight: 500, scrollTop: 100, clientHeight: 400 });
      fireEvent.scroll(scroller);
      act(() => virtualTranscriptMock.totalExtentChanged?.(500));
      virtualTranscriptMock.scrollToTail.mockClear();

      vi.setSystemTime(1050);
      setupScroller(scroller, { scrollHeight: 500, scrollTop: 80, clientHeight: 400 });
      fireEvent.scroll(scroller);
      act(() => virtualTranscriptMock.pinnedChanged?.(true));

      vi.setSystemTime(1100);
      fireEvent.touchStart(scroller, { touches: [{}] });
      vi.setSystemTime(1110);
      fireEvent.touchEnd(scroller, { touches: [] });
      vi.setSystemTime(1150);
      setupScroller(scroller, { scrollHeight: 600, scrollTop: 100, clientHeight: 400 });
      act(() => virtualTranscriptMock.totalExtentChanged?.(600));

      expect(virtualTranscriptMock.scrollToTail).toHaveBeenCalled();
      expect(container.querySelector('.jump-to-newest')).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });


  it('does NOT re-snap after a second iOS braking touch with sparse scroll events', () => {
    vi.useFakeTimers();
    try {
      const historical = Array.from({ length: 5 }, (_, i) => makeMessage(i + 1, 'user'));
      const subAgentsState: ConversationState = {
        type: 'awaiting_sub_agents',
        pending: [],
        completed_results: [],
      };
      const { container } = render(
        withConvContext(
          <MessageList
            messages={historical}
            pendingMessages={[]}
            convState={subAgentsState}
            onRetry={vi.fn()}
            onOpenFile={undefined}
            conversationId="conv-ios-brake"

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
        ),
      );

      const scroller = container.querySelector<HTMLElement>('#messages')!;
      vi.setSystemTime(1000);
      setupScroller(scroller, { scrollHeight: 500, scrollTop: 100, clientHeight: 400 });
      fireEvent.scroll(scroller);
      act(() => virtualTranscriptMock.totalExtentChanged?.(500));
      virtualTranscriptMock.scrollToTail.mockClear();

      vi.setSystemTime(1050);
      fireEvent.touchStart(scroller, { touches: [{}] });
      vi.setSystemTime(1070);
      fireEvent.touchMove(scroller, { touches: [{}] });
      vi.setSystemTime(1080);
      fireEvent.touchEnd(scroller, { touches: [] });
      vi.setSystemTime(1100);
      setupScroller(scroller, { scrollHeight: 500, scrollTop: 80, clientHeight: 400 });
      fireEvent.scroll(scroller);
      act(() => virtualTranscriptMock.pinnedChanged?.(false));

      vi.setSystemTime(1800);
      fireEvent.touchStart(scroller, { touches: [{}] });
      vi.setSystemTime(1820);
      fireEvent.touchMove(scroller, { touches: [{}] });
      vi.setSystemTime(1830);
      fireEvent.touchEnd(scroller, { touches: [] });

      vi.setSystemTime(1900);
      setupScroller(scroller, { scrollHeight: 600, scrollTop: 80, clientHeight: 400 });
      act(() => virtualTranscriptMock.totalExtentChanged?.(600));
      expect(virtualTranscriptMock.scrollToTail).not.toHaveBeenCalled();
      expect(container.querySelector('.jump-to-newest')).not.toBeNull();

      vi.setSystemTime(3101);
      setupScroller(scroller, { scrollHeight: 700, scrollTop: 200, clientHeight: 400 });
      act(() => virtualTranscriptMock.totalExtentChanged?.(700));
      expect(virtualTranscriptMock.scrollToTail).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it('does NOT re-snap after a moved touch before any upward scroll event', () => {
    vi.useFakeTimers();
    try {
      const historical = Array.from({ length: 5 }, (_, i) => makeMessage(i + 1, 'user'));
      const subAgentsState: ConversationState = {
        type: 'awaiting_sub_agents',
        pending: [],
        completed_results: [],
      };
      const { container } = render(
        withConvContext(
          <MessageList
            messages={historical}
            pendingMessages={[]}
            convState={subAgentsState}
            onRetry={vi.fn()}
            onOpenFile={undefined}
            conversationId="conv-touchmove-no-scroll"

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
        ),
      );

      const scroller = container.querySelector<HTMLElement>('#messages')!;
      vi.setSystemTime(1000);
      setupScroller(scroller, { scrollHeight: 500, scrollTop: 100, clientHeight: 400 });
      fireEvent.scroll(scroller);
      act(() => virtualTranscriptMock.totalExtentChanged?.(500));
      act(() => virtualTranscriptMock.pinnedChanged?.(false));
      virtualTranscriptMock.scrollToTail.mockClear();

      vi.setSystemTime(1050);
      fireEvent.touchStart(scroller, { touches: [{}] });
      vi.setSystemTime(1060);
      fireEvent.touchMove(scroller, { touches: [{}] });
      vi.setSystemTime(1070);
      fireEvent.touchEnd(scroller, { touches: [] });
      vi.setSystemTime(1100);
      setupScroller(scroller, { scrollHeight: 600, scrollTop: 95, clientHeight: 400 });
      act(() => virtualTranscriptMock.totalExtentChanged?.(600));

      expect(virtualTranscriptMock.scrollToTail).not.toHaveBeenCalled();
      expect(container.querySelector('.jump-to-newest')).not.toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it('does NOT re-snap when at-bottom fires during a moved touch before touch end', () => {
    vi.useFakeTimers();
    try {
      const historical = Array.from({ length: 5 }, (_, i) => makeMessage(i + 1, 'user'));
      const subAgentsState: ConversationState = {
        type: 'awaiting_sub_agents',
        pending: [],
        completed_results: [],
      };
      const { container } = render(
        withConvContext(
          <MessageList
            messages={historical}
            pendingMessages={[]}
            convState={subAgentsState}
            onRetry={vi.fn()}
            onOpenFile={undefined}
            conversationId="conv-atbottom-during-touchmove"

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
        ),
      );

      const scroller = container.querySelector<HTMLElement>('#messages')!;
      vi.setSystemTime(1000);
      setupScroller(scroller, { scrollHeight: 500, scrollTop: 100, clientHeight: 400 });
      fireEvent.scroll(scroller);
      act(() => virtualTranscriptMock.totalExtentChanged?.(500));
      virtualTranscriptMock.scrollToTail.mockClear();

      vi.setSystemTime(1050);
      fireEvent.touchStart(scroller, { touches: [{}] });
      vi.setSystemTime(1060);
      fireEvent.touchMove(scroller, { touches: [{}] });
      act(() => virtualTranscriptMock.pinnedChanged?.(true));
      vi.setSystemTime(1070);
      fireEvent.touchEnd(scroller, { touches: [] });
      vi.setSystemTime(1100);
      setupScroller(scroller, { scrollHeight: 600, scrollTop: 95, clientHeight: 400 });
      act(() => virtualTranscriptMock.totalExtentChanged?.(600));

      expect(virtualTranscriptMock.scrollToTail).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });


  it('maps touchcancel after movement to durable reading ownership', () => {
    const historical = Array.from({ length: 5 }, (_, i) => makeMessage(i + 1, 'user'));
    const { container } = render(
      withConvContext(
        <MessageList
          messages={historical}
          pendingMessages={[]}
          convState={{ type: 'awaiting_sub_agents', pending: [], completed_results: [] }}
          onRetry={vi.fn()}
          onOpenFile={undefined}
          conversationId="conv-touchcancel"

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
      ),
    );

    const scroller = container.querySelector<HTMLElement>('#messages')!;
    setupScroller(scroller, { scrollHeight: 500, scrollTop: 100, clientHeight: 400 });
    act(() => virtualTranscriptMock.totalExtentChanged?.(500));
    virtualTranscriptMock.scrollToTail.mockClear();

    fireEvent.touchStart(scroller, { touches: [{}] });
    fireEvent.touchMove(scroller, { touches: [{}] });
    fireEvent.touchCancel(scroller, { touches: [] });
    setupScroller(scroller, { scrollHeight: 600, scrollTop: 95, clientHeight: 400 });
    act(() => virtualTranscriptMock.totalExtentChanged?.(600));

    expect(virtualTranscriptMock.scrollToTail).not.toHaveBeenCalled();
    expect(container.querySelector('.jump-to-newest')).not.toBeNull();
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

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
      ),
    );

    const scroller = container.querySelector<HTMLElement>('#messages')!;
    setupScroller(scroller, { scrollHeight: 500, scrollTop: 50, clientHeight: 400 });
    act(() => virtualTranscriptMock.totalExtentChanged?.(500));
    // Engage so the assertion exercises the pin branch, not the settle rescue.
    fireEvent.wheel(scroller, { deltaY: 50 });
    virtualTranscriptMock.scrollToTail.mockClear();

    // scrollTop increases (downward) — e.g. our own snap or the user heading
    // to the bottom. Must NOT suppress auto-follow.
    setupScroller(scroller, { scrollHeight: 500, scrollTop: 100, clientHeight: 400 });
    fireEvent.scroll(scroller);

    setupScroller(scroller, { scrollHeight: 600, scrollTop: 100, clientHeight: 400 });
    // oldFromBottom = 500 - 100 - 400 = 0 (pinned)
    act(() => virtualTranscriptMock.totalExtentChanged?.(600));
    expect(virtualTranscriptMock.scrollToTail).toHaveBeenCalled();
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

          transcriptPositioning={{ kind: 'idle', view: { conversationId: 'conv-under-test', generation: 1, transcriptGeneration: 1 } }}/>,
      ),
    );

    const scroller = container.querySelector<HTMLElement>('#messages')!;
    // Seed baseline: user at bottom with tall viewport
    setupScroller(scroller, { scrollHeight: 800, scrollTop: 100, clientHeight: 700 });
    act(() => virtualTranscriptMock.totalExtentChanged?.(800));
    // oldFromBottom = 800 - 100 - 700 = 0 (pinned)
    // Engage (downward wheel) so the assertion exercises the pin branch,
    // not the pre-engagement settle rescue.
    fireEvent.wheel(scroller, { deltaY: 50 });

    // Clear so we only observe subsequent calls
    virtualTranscriptMock.scrollToTail.mockClear();

    // Viewport shrinks from 700 to 500 (200px shrink > 100px threshold)
    // Without viewport-shrink handling, oldFromBottom = 800 - 100 - 500 = 200 > 100
    // With shrink handling, uses prevClientHeight (700): oldFromBottom = 800 - 100 - 700 = 0 <= 100
    setupScroller(scroller, { scrollHeight: 800, scrollTop: 100, clientHeight: 500 });
    act(() => virtualTranscriptMock.totalExtentChanged?.(800));

    expect(virtualTranscriptMock.scrollToTail).toHaveBeenCalled();
  });
});


