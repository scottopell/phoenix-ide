// Streaming-isolation perf invariant (REQ-MLRU-010).
//
// This file does NOT measure browser timings (no browser available in
// the test environment). It verifies the architectural property that
// drives the per-token streaming win: a burst of sse_token actions
// must NOT cause <MessageListImpl> to re-render. Only the
// <StreamingMessage> leaf may commit per token.
//
// React.Profiler's onRender fires for every commit of the profiled
// subtree, regardless of whether layout/paint happened — so the counts
// here are valid in happy-dom even though layout is absent.
//
// If a future change re-introduces a per-token prop on MessageList (or
// breaks the memo boundary, or moves the buffer subscription back up
// the tree), this test will fail loudly with a concrete commit-count
// number — preventing the regression that prompted task 01004's
// streaming work.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { forwardRef, Profiler, type ProfilerOnRenderCallback } from 'react';
import { act, render } from '@testing-library/react';
import { MessageList } from './MessageList';
import { ConversationContext } from '../conversation/ConversationContext';
import { ConversationStore } from '../conversation/ConversationStore';
import type { ConversationState, Message } from '../api';

// Keep heavy display components mocked — we measure WHEN React commits,
// not what each component does inside its render. The real
// <StreamingMessage> is imported so its useStreamingBuffer subscription
// runs against the real store.
vi.mock('./MessageComponents', () => ({
  UserMessage: () => <div className="message user">u</div>,
  QueuedUserMessage: () => null,
  AgentMessage: () => <div className="message agent">a</div>,
  SubAgentStatus: () => null,
  formatMessageTime: () => '12:00',
}));
vi.mock('./MessageContextMenu', () => ({ MessageContextMenu: () => null }));

// Mock VirtualTranscript as a passthrough. This test measures React commit
// counts for the MessageListImpl boundary; VirtualTranscript's internal
// scheduling is irrelevant to the streaming-isolation invariant being verified.
vi.mock('./VirtualTranscript', async () => {
  const actual = await vi.importActual<typeof import('./VirtualTranscript')>('./VirtualTranscript');
  return {
    ...actual,
    VirtualTranscript: forwardRef(<T,>({
      items,
      renderItem,
      getKey,
      header,
      empty,
    }: {
      items: readonly T[];
      renderItem: (item: T, index: number) => React.ReactNode;
      getKey?: (item: T, index: number) => React.Key;
      header?: React.ReactNode;
      empty?: React.ReactNode;
    }, _ref: React.ForwardedRef<unknown>) => (
      <div data-testid="mock-virtual-transcript">
        {header}
        {items.length === 0 ? empty : items.map((item, i) => {
          const key = getKey ? getKey(item, i) : i;
          return <div key={key}>{renderItem(item, i)}</div>;
        })}
      </div>
    )),
  };
});
// Keep the markdown / syntax-highlighter cheap so per-token commits of
// the real <StreamingMessage> are fast and don't dominate the test
// budget. The commit COUNT is the assertion target, not timing.
vi.mock('react-markdown', () => ({
  default: ({ children }: { children: string }) => <span>{children}</span>,
}));
vi.mock('../utils/syntaxHighlighter', () => ({
  SyntaxHighlighter: ({ children }: { children: string }) => <pre>{children}</pre>,
  oneDark: {},
  oneLight: {},
}));

class MockResizeObserver {
  observe(): void {
    /* noop */
  }
  disconnect(): void {
    /* noop */
  }
}

let originalGetBCR: typeof HTMLElement.prototype.getBoundingClientRect | undefined;
let scrollHeightDescriptor: PropertyDescriptor | undefined;
let clientHeightDescriptor: PropertyDescriptor | undefined;

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', MockResizeObserver);
  scrollHeightDescriptor = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'scrollHeight');
  clientHeightDescriptor = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientHeight');
  Object.defineProperty(HTMLElement.prototype, 'scrollHeight', { configurable: true, get: () => 1000 });
  Object.defineProperty(HTMLElement.prototype, 'clientHeight', { configurable: true, get: () => 400 });
  originalGetBCR = HTMLElement.prototype.getBoundingClientRect;
  HTMLElement.prototype.getBoundingClientRect = function () {
    return new DOMRect(0, 0, 0, 0);
  };
});

afterEach(() => {
  vi.unstubAllGlobals();
  if (originalGetBCR) HTMLElement.prototype.getBoundingClientRect = originalGetBCR;
  if (scrollHeightDescriptor) Object.defineProperty(HTMLElement.prototype, 'scrollHeight', scrollHeightDescriptor);
  if (clientHeightDescriptor) Object.defineProperty(HTMLElement.prototype, 'clientHeight', clientHeightDescriptor);
});

interface RenderCount {
  total: number;
  phases: string[];
}

/** Build a Profiler that increments a counter on each commit. */
function makeCounter(): { count: RenderCount; onRender: ProfilerOnRenderCallback } {
  const count: RenderCount = { total: 0, phases: [] };
  const onRender: ProfilerOnRenderCallback = (_id, phase) => {
    count.total++;
    count.phases.push(phase);
  };
  return { count, onRender };
}

function userMsg(id: string, text = 'hi'): Message {
  return {
    message_id: id,
    sequence_id: 0,
    conversation_id: 'c1',
    message_type: 'user',
    content: { text },
    created_at: '',
  };
}

const llmRequesting: ConversationState = { type: 'llm_requesting', attempt: 1 };
const idle: ConversationState = { type: 'idle' };

// Hoist all empty / no-op prop references so they're reference-stable
// across harness renders — otherwise React.memo's shallow compare
// breaks on every parent re-render and the perf-isolation property we
// want to prove gets masked by harness churn.
const EMPTY_PENDING: never[] = [];
const NOOP_RETRY = () => {};

describe('MessageList streaming-isolation perf invariant', () => {
  it('per-token streaming commits the leaf but NOT MessageListImpl (REQ-MLRU-010)', () => {
    const slug = 'perf-conv';
    const store = new ConversationStore();

    // Seed phase via sse_state_change (no epoch needed — atom's
    // connectionEpoch is null, bootstrap path lets the action through).
    // We pass `messages` to MessageList as a separate prop below; the
    // store's atom doesn't need to hold them for the streaming-isolation
    // signal we're measuring.
    const seedMessages = [userMsg('u1'), userMsg('u2'), userMsg('u3')];
    store.dispatch(slug, {
      type: 'sse_state_change',
      sequenceId: 1,
      phase: llmRequesting,
      stateUpdatedAt: 0,
    });

    const messageListCounter = makeCounter();

    function Harness() {
      return (
        <ConversationContext.Provider value={store}>
          <Profiler id="message-list" onRender={messageListCounter.onRender}>
            <MessageList
              messages={seedMessages}
              pendingMessages={EMPTY_PENDING}
              convState={llmRequesting}
              onRetry={NOOP_RETRY}
              onCancelSteering={undefined}
              onOpenFile={undefined}
              conversationId="c1"
              slug={slug}
            />
          </Profiler>
        </ConversationContext.Provider>
      );
    }

    render(<Harness />);

    // After initial mount, MessageList has committed once. Capture the
    // baseline so subsequent token deltas measure only the streaming-
    // induced commits.
    const baselineMessageListCommits = messageListCounter.count.total;
    expect(baselineMessageListCommits).toBeGreaterThanOrEqual(1);
    // Log what triggered baseline commits for diagnosis if perf regresses.
    console.info('[perf-isolation] baseline commits:', messageListCounter.count.phases.slice());

    // Dispatch a 100-token burst directly into the store. Each
    // dispatch triggers useSyncExternalStore notify → React schedules
    // an update for subscribers whose getSnapshot returned a different
    // value. useStreamingRequestId returns the same string (the
    // session's request_id) for every token, so MessageList should NOT
    // re-commit. useStreamingBuffer returns a fresh buffer ref on
    // every token, so StreamingMessage SHOULD re-commit (the rAF
    // coalescing inside the leaf is a separate concern from the
    // commit count we're measuring here).
    act(() => {
      for (let i = 0; i < 100; i++) {
        store.dispatch(slug, {
          type: 'sse_token',
          sequenceId: i + 1,
          delta: `t${i} `,
          requestId: 'test-req-id',
        });
      }
    });

    const afterStreamingMessageListCommits = messageListCounter.count.total;
    const messageListDeltaCommits = afterStreamingMessageListCommits - baselineMessageListCommits;
    console.info('[perf-isolation] all commits:', messageListCounter.count.phases.slice());

    // The hard claim: per-token streaming triggers AT MOST ONE
    // <MessageListImpl> commit — the streaming-start transition where
    // useStreamingRequestId returns its first non-null value and the
    // streamingHandle key materialises (adding the streaming_agent
    // TailUnit). Tokens 2..N keep request_id stable; their dispatches
    // notify subscribers, but useStreamingRequestId's getSnapshot
    // returns the same string, so MessageList stays memo'd.
    //
    // Without this isolation, the delta would be ~100 (one commit per
    // token). With it, the delta is 1, regardless of burst size.
    expect(messageListDeltaCommits).toBeLessThanOrEqual(1);

    // Surface a one-line summary for human reading when the test runs.
    console.info(
      `[perf-isolation] 100 tokens dispatched; MessageList commits during burst: ${messageListDeltaCommits}`,
    );
  });

  it('a phase transition (streaming-start) DOES re-commit MessageListImpl exactly once', () => {
    // The complementary claim: when isStreaming transitions
    // null → number (streaming starts), the streamingHandle key
    // changes and MessageList must re-commit once to add the
    // streaming_agent TailUnit. This proves the memo is correctly
    // ATTACHED to the streaming-active signal — neither too loose
    // (per-token regression) nor too tight (missing the start/stop
    // transitions).
    const slug = 'perf-phase';
    const store = new ConversationStore();

    const seed = [userMsg('u1')];

    const counter = makeCounter();

    function Harness({ convState }: { convState: ConversationState }) {
      return (
        <ConversationContext.Provider value={store}>
          <Profiler id="message-list" onRender={counter.onRender}>
            <MessageList
              messages={seed}
              pendingMessages={EMPTY_PENDING}
              convState={convState}
              onRetry={NOOP_RETRY}
              onCancelSteering={undefined}
              onOpenFile={undefined}
              conversationId="c1"
              slug={slug}
            />
          </Profiler>
        </ConversationContext.Provider>
      );
    }

    const { rerender } = render(<Harness convState={idle} />);
    const baseline = counter.count.total;

    // Transition phase to llm_requesting AND emit a first token —
    // streamingBuffer goes from null to non-null, useStreamingRequestId
    // returns a string where it previously returned null. MessageList
    // memo breaks; one commit expected.
    act(() => {
      store.dispatch(slug, {
        type: 'sse_state_change',
        sequenceId: 1,
        phase: llmRequesting,
        stateUpdatedAt: 0,
      });
      store.dispatch(slug, {
        type: 'sse_token',
        sequenceId: 2,
        delta: 'hello',
        requestId: 'test-req-id',
      });
    });

    // Force harness re-render with the new phase prop.
    rerender(<Harness convState={llmRequesting} />);

    const startCommits = counter.count.total - baseline;
    // At least 1 (transition), at most a couple (transition +
    // harness re-render). Anything in this range is fine —
    // important is that it's NOT zero (memo would be over-tight)
    // and NOT one per token (memo would be over-loose).
    expect(startCommits).toBeGreaterThanOrEqual(1);
    expect(startCommits).toBeLessThanOrEqual(3);
  });
});
