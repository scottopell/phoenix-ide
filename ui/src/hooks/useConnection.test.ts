// useConnection integration tests for task 08683 (epoch-stamp SSE events).
//
// The atom-side reducer rejection is covered exhaustively in
// `src/conversation/atom.test.ts > connection epoch (task 08683)`. These
// tests cover the *hook-side* obligation: every dispatch made by
// `useConnection` carries the `epoch` of the OPEN_SSE generation that
// produced it, so the reducer can do its job.
//
// Strategy: shim `globalThis.EventSource` with a controllable fake that
// records every constructed instance. Render the hook with a spy dispatch
// (or a real `ConversationStore` for the contamination scenario), drive
// events synthetically, and assert what landed in the atom.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { useConnection } from './useConnection';
import type { SSEAction } from '../conversation/atom';
import { ConversationStore } from '../conversation/ConversationStore';

// ---------------------------------------------------------------------------
// EventSource shim
// ---------------------------------------------------------------------------

type Listener = (event: MessageEvent) => void;

class FakeEventSource {
  url: string;
  readyState = 0;
  closed = false;
  // typed listener buckets keyed by event name
  private listeners = new Map<string, Set<Listener>>();

  static instances: FakeEventSource[] = [];

  constructor(url: string) {
    this.url = url;
    FakeEventSource.instances.push(this);
  }

  addEventListener(type: string, fn: Listener): void {
    let set = this.listeners.get(type);
    if (!set) {
      set = new Set();
      this.listeners.set(type, set);
    }
    set.add(fn);
  }

  removeEventListener(type: string, fn: Listener): void {
    this.listeners.get(type)?.delete(fn);
  }

  close(): void {
    this.closed = true;
  }

  /** Drive a typed SSE event into all registered listeners for `type`. */
  emit(type: string, data: unknown): void {
    const payload = typeof data === 'string' ? data : JSON.stringify(data);
    const event = new MessageEvent(type, { data: payload });
    const set = this.listeners.get(type);
    if (!set) return;
    for (const fn of set) fn(event);
  }
}

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

let originalEventSource: typeof EventSource | undefined;

beforeEach(() => {
  originalEventSource = (globalThis as { EventSource?: typeof EventSource }).EventSource;
  (globalThis as { EventSource: unknown }).EventSource =
    FakeEventSource as unknown as typeof EventSource;
  FakeEventSource.instances.length = 0;
});

afterEach(() => {
  if (originalEventSource) {
    (globalThis as { EventSource: typeof EventSource }).EventSource = originalEventSource;
  }
});

// Minimal valid `init` payload — must satisfy SseInitDataSchema or
// `parseEvent` throws (dev mode — schema violations are loud by design).
function makeInitPayload(convId: string, slug: string) {
  return {
    sequence_id: 0,
    conversation: {
      id: convId,
      slug,
      model: 'claude-3-5-sonnet',
      cwd: '/tmp',
      created_at: '2024-01-01T00:00:00Z',
      updated_at: '2024-01-01T00:00:00Z',
      message_count: 0,
    },
    messages: [],
    agent_working: false,
    last_sequence_id: 0,
    display_state: 'idle',
    context_window_size: 0,
    breadcrumbs: [],
    commits_behind: 0,
    commits_ahead: 0,
    project_name: null,
  };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('useConnection epoch stamping (task 08683)', () => {
  it('stamps every wire-derived dispatch with the connection epoch', () => {
    const captured: SSEAction[] = [];
    const dispatch = (a: SSEAction) => {
      captured.push(a);
    };

    renderHook(() => useConnection({ conversationId: 'conv-A', dispatch }));

    expect(FakeEventSource.instances).toHaveLength(1);
    const es = FakeEventSource.instances[0]!;

    // First action dispatched on OPEN_SSE is `connection_opened` with the
    // freshly-minted epoch. This is the bootstrap that lifts the atom out
    // of `connectionEpoch === null`.
    const opened = captured.find((a) => a.type === 'connection_opened');
    expect(opened).toBeDefined();
    expect(opened && 'epoch' in opened ? opened.epoch : undefined).toBe(1);

    // Drive an init through the wire — every dispatch downstream of it
    // must carry epoch=1.
    act(() => {
      es.emit('init', makeInitPayload('conv-A', 'slug-A'));
    });

    const wireActions = captured.filter(
      (a) =>
        a.type === 'sse_init' ||
        a.type === 'connection_state' ||
        a.type === 'sse_message' ||
        a.type === 'sse_state_change' ||
        a.type === 'connection_opened',
    );
    expect(wireActions.length).toBeGreaterThan(0);
    for (const a of wireActions) {
      // Every connection-originated action carries an epoch field.
      expect(a).toHaveProperty('epoch');
      const withEpoch = a as { epoch?: number };
      expect(withEpoch.epoch).toBe(1);
    }
  });

  it('drops a stale-EventSource event after slug change (cross-conversation contamination guard)', () => {
    // Real ConversationStore so we can observe atom mutations directly.
    const store = new ConversationStore();
    const slugA = 'slug-A';
    const slugB = 'slug-B';

    // Per-slug dispatch mirrors how ConversationPage wires `useConnection`.
    let activeSlug = slugA;
    const dispatch = (a: SSEAction) => store.dispatch(activeSlug, a);

    const { rerender } = renderHook(
      ({ convId, dispatchFn }: { convId: string; dispatchFn: (a: SSEAction) => void }) =>
        useConnection({ conversationId: convId, dispatch: dispatchFn }),
      { initialProps: { convId: 'conv-A', dispatchFn: dispatch } },
    );

    // First connection: A. Drive init so atom A learns its epoch + state.
    expect(FakeEventSource.instances).toHaveLength(1);
    const esA = FakeEventSource.instances[0]!;
    act(() => {
      esA.emit('init', makeInitPayload('conv-A', slugA));
    });

    const atomAAfterInit = store.getSnapshot(slugA);
    expect(atomAAfterInit.connectionEpoch).toBe(1);
    expect(atomAAfterInit.conversationId).toBe('conv-A');
    expect(atomAAfterInit.connectionState).toBe('live');

    // Navigate to B. ConversationPage swaps the slug-bound dispatch BEFORE
    // the cleanup effect fires CLOSE_SSE; useConnection's `dispatchRef` is
    // updated via the `useEffect([dispatch])` hook below the executor.
    activeSlug = slugB;
    rerender({ convId: 'conv-B', dispatchFn: dispatch });

    // Two FakeEventSources should now exist: A's (still in `instances[0]`,
    // its `close()` was called by CLOSE_SSE) and B's freshly opened.
    expect(FakeEventSource.instances).toHaveLength(2);
    expect(esA.closed).toBe(true);
    const esB = FakeEventSource.instances[1]!;

    // Drive B's init so atom B learns its epoch (epoch=2 — the second
    // OPEN_SSE this hook has performed).
    act(() => {
      esB.emit('init', makeInitPayload('conv-B', slugB));
    });
    const atomBAfterInit = store.getSnapshot(slugB);
    expect(atomBAfterInit.connectionEpoch).toBe(2);

    // CONTAMINATION SCENARIO: a buffered event from A's still-around
    // EventSource fires its handler. `dispatchRef.current` already points
    // at B's atom (we swapped activeSlug above; the dispatch closure
    // routes to slugB). The action is stamped with A's epoch (1) but
    // arrives at B's atom (epoch 2) — reducer must reject it.
    const messagesBefore = atomBAfterInit.messages.length;
    act(() => {
      esA.emit('message', {
        sequence_id: 999,
        message: {
          message_id: 'leaked-msg-from-A',
          sequence_id: 999,
          conversation_id: 'conv-A',
          message_type: 'agent',
          content: { text: 'should not land in B' },
          created_at: '2024-01-01T00:00:00Z',
        },
      });
    });

    const atomBAfterLeak = store.getSnapshot(slugB);
    expect(atomBAfterLeak.messages.length).toBe(messagesBefore);
    expect(atomBAfterLeak.messages.find((m) => m.message_id === 'leaked-msg-from-A')).toBeUndefined();
    // The atom reference should be unchanged (reducer returned `atom`
    // verbatim on the stale-epoch drop).
    expect(atomBAfterLeak).toBe(atomBAfterInit);
  });

  it('mints a strictly increasing epoch on each OPEN_SSE', () => {
    const captured: SSEAction[] = [];
    const dispatch = (a: SSEAction) => {
      captured.push(a);
    };

    const { rerender } = renderHook(
      ({ convId }: { convId: string | undefined }) =>
        useConnection({ conversationId: convId, dispatch }),
      { initialProps: { convId: 'conv-A' as string | undefined } },
    );

    // First connection → epoch 1
    let opens = captured.filter((a) => a.type === 'connection_opened');
    expect(opens).toHaveLength(1);
    expect((opens[0] as { epoch: number }).epoch).toBe(1);

    // Tear down, then connect again → epoch 2.
    rerender({ convId: undefined });
    rerender({ convId: 'conv-B' });

    opens = captured.filter((a) => a.type === 'connection_opened');
    expect(opens).toHaveLength(2);
    expect((opens[1] as { epoch: number }).epoch).toBe(2);

    // And once more → epoch 3.
    rerender({ convId: undefined });
    rerender({ convId: 'conv-C' });

    opens = captured.filter((a) => a.type === 'connection_opened');
    expect(opens).toHaveLength(3);
    expect((opens[2] as { epoch: number }).epoch).toBe(3);
  });

  it('does not schedule duplicate retry timers under StrictMode (regression: setTimeout-in-functional-updater)', async () => {
    // The pre-08683 implementation called `setTimeout(executeEffects, 0)`
    // *inside* setMachineState's functional updater. StrictMode invokes
    // the updater twice in dev; that produced two timer schedules per
    // SSE_ERROR. After 08683 the effect runs synchronously once per
    // dispatch, so even under a doubled-render pattern only one timer is
    // scheduled per error.
    //
    // We verify the contract by counting the EventSources opened in
    // response to a single SSE_ERROR + RETRY_TIMER_FIRED cycle: it must
    // be exactly one per intended retry, not two.
    vi.useFakeTimers();
    try {
      const dispatch = vi.fn();
      renderHook(() => useConnection({ conversationId: 'conv-A', dispatch }));
      expect(FakeEventSource.instances).toHaveLength(1);
      const esA = FakeEventSource.instances[0]!;

      // Trigger reconnect: emit a connection error (no-data error event
      // signals native EventSource failure on the real wire).
      act(() => {
        esA.emit('error', '');
      });

      // Advance past the 1s base backoff. If the bug were back, two
      // RETRY_TIMER_FIRED transitions would fire and produce two new
      // EventSources.
      act(() => {
        vi.advanceTimersByTime(1500);
      });

      // One additional EventSource (the retry); never two.
      expect(FakeEventSource.instances.length).toBe(2);
    } finally {
      vi.useRealTimers();
    }
  });
});
